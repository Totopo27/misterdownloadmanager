//! Adaptive streams for the browser tests: an HLS master playlist with two video variants and an
//! audio rendition, and a DASH manifest with the same tracks (and a copy marked as encrypted with
//! Widevine, for the extension's media list to show as protected, #38). Each track has `SEGMENTS` segments
//! of `SEGMENT_SECS` seconds. HLS segments are real MPEG-TS (`ts_segment`), so ffmpeg (yt-dlp's
//! HLS downloader and its merger) finds their streams and copies them, as it does a site's (#236:
//! fake bytes made every ffmpeg stop with "could not find codec parameters" or "Output file does
//! not contain any stream"): the audio track is AAC silence, each video track an H.264 grey
//! picture (`h264`) and nothing else, as a site's are when its sound is a rendition of its own
//! (a video track that held only sound failed yt-dlp's merge, `-map 0:v:0`). DASH segments are
//! deterministic fakes (init segments start with an `ftyp` box), enough for a client that lists,
//! fetches or joins them, not for one that decodes them.

use std::collections::HashMap;

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::{content, h264};

pub const SEGMENTS: u32 = 3;
pub const SEGMENT_SECS: u32 = 4;
const TS_PACKET: usize = 188;

/// A track: its name in paths, video size (none for audio), bandwidth, bytes per segment.
struct Track {
    name: &'static str,
    video: Option<(u32, u32)>,
    bandwidth: u32,
    segment_len: usize,
}

const TRACKS: [Track; 3] = [
    Track {
        name: "720p",
        video: Some((1280, 720)),
        bandwidth: 1_280_000,
        segment_len: TS_PACKET * 256,
    },
    Track {
        name: "360p",
        video: Some((640, 360)),
        bandwidth: 640_000,
        segment_len: TS_PACKET * 128,
    },
    Track {
        name: "audio",
        video: None,
        bandwidth: 128_000,
        segment_len: TS_PACKET * 32,
    },
];

fn track(name: &str) -> Option<(usize, &'static Track)> {
    TRACKS.iter().enumerate().find(|(_, t)| t.name == name)
}

fn text(ty: &'static str, body: String) -> Response {
    ([(header::CONTENT_TYPE, ty)], body).into_response()
}

fn bytes(ty: &'static str, body: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, ty)], body).into_response()
}

/// Segment `index` of track number `track`: seeded content, distinct per track and segment.
fn segment(track: usize, index: u32, len: usize) -> Vec<u8> {
    let mut buf = vec![0; len];
    content::fill(10_000 + track as u64 * 100 + index as u64, 0, &mut buf);
    buf
}

/// The PMT's PID, the sound's, the picture's, and the null packets' (ISO/IEC 13818-1).
const PMT_PID: u16 = 0x1000;
const AUDIO_PID: u16 = 0x0100;
const VIDEO_PID: u16 = 0x0101;
const NULL_PID: u16 = 0x1fff;
/// AAC-LC, 32 kHz, mono: 1024 samples a frame, so a 4 s segment is exactly 125 frames.
const RATE: u32 = 32_000;
const FRAMES: u32 = SEGMENT_SECS * RATE / 1024;
/// One AAC-LC frame of digital silence (a single channel element, no spectral data).
const SILENCE: [u8; 4] = [0x01, 0x18, 0x20, 0x07];
/// 90 kHz clock ticks per AAC frame.
const FRAME_TICKS: u64 = 1024 * 90_000 / RATE as u64;
/// The video tracks' frames a second, and their 90 kHz ticks per frame.
const FPS: u32 = 25;
const VIDEO_TICKS: u64 = 90_000 / FPS as u64;

/// A PES of stream `id` with `pts` and `payload`, its length filled in.
fn pes(id: u8, pts: u64, payload: &[u8]) -> Vec<u8> {
    let mut pes = vec![0x00, 0x00, 0x01, id, 0, 0, 0x80, 0x80, 5];
    pes.extend(timestamp(0x2, pts));
    pes.extend(payload);
    let pes_len = u16::try_from(pes.len() - 6).expect("a PES fits its length field");
    pes[4..6].copy_from_slice(&pes_len.to_be_bytes());
    pes
}

/// Segment `index` of track number `track` as MPEG-TS, exactly `len` bytes (a multiple of 188):
/// a PAT, a PMT, then the track's stream, whose PTS goes on from the segment before: the sound's
/// one PES of `FRAMES` silent ADTS frames, or a picture's PES a frame (`FPS` a second, the first
/// an IDR frame), each PES's first packet with a PCR. Then null packets with seeded payloads, so
/// every segment of every track differs. Continuity counters go on from the segment before, as
/// in one stream cut in pieces.
fn ts_segment(track: usize, index: u32, len: usize) -> Vec<u8> {
    let start = u64::from(index) * u64::from(SEGMENT_SECS) * 90_000;
    // The stream's PID, its PMT type, and its PESes with their PTS.
    let (pid, kind, pieces) = match TRACKS[track].video {
        Some((w, h)) => {
            // idr_pic_id 1 or 2: two IDR frames in a row differ in it.
            let idr = index % 2 + 1;
            let frames = (0..SEGMENT_SECS * FPS).map(|n| {
                let pts = start + u64::from(n) * VIDEO_TICKS;
                (pts, pes(0xe0, pts, &h264::frame(w, h, FPS, idr, n)))
            });
            (VIDEO_PID, 0x1b, frames.collect::<Vec<_>>())
        }
        None => {
            debug_assert_eq!(start, u64::from(index) * u64::from(FRAMES) * FRAME_TICKS);
            let frame_len = 7 + SILENCE.len();
            let mut sound = Vec::new();
            for _ in 0..FRAMES {
                sound.extend(adts_header(frame_len));
                sound.extend(SILENCE);
            }
            (AUDIO_PID, 0x0f, vec![(start, pes(0xc0, start, &sound))])
        }
    };

    let per_segment: usize = pieces.iter().map(|(_, p)| packet_count(p.len())).sum();
    let cc = |per_segment: usize| ((index as usize * per_segment) % 16) as u8;
    let mut out = Vec::with_capacity(len);
    out.extend(psi_packet(0, cc(1), &pat()));
    out.extend(psi_packet(PMT_PID, cc(1), &pmt(kind, pid)));
    let mut n = 0;
    for (i, (pts, pes)) in pieces.iter().enumerate() {
        // The sound's one PES is a random access point, a picture's first frame only.
        let key = i == 0;
        let mut rest = &pes[..];
        let mut first = true;
        while !rest.is_empty() {
            let counter = (cc(per_segment) + n as u8) & 15;
            let pcr = first.then_some((*pts, key));
            let (packet, used) = pes_packet(pid, first, counter, pcr, rest);
            out.extend(packet);
            rest = &rest[used..];
            first = false;
            n += 1;
        }
    }
    debug_assert_eq!(n, per_segment);
    assert!(out.len() <= len, "a {len}-byte segment holds its stream");
    let mut fill = segment(track, index, len - out.len());
    for (k, packet) in fill.chunks_mut(TS_PACKET).enumerate() {
        let head = [
            0x47,
            (NULL_PID >> 8) as u8,
            NULL_PID as u8,
            0x10 | (k % 16) as u8,
        ];
        packet[..4].copy_from_slice(&head);
    }
    out.extend(fill);
    out
}

/// TS packets a PES of `len` bytes takes: 176 bytes in the first (its adaptation field holds
/// the PCR), 184 in each after.
fn packet_count(len: usize) -> usize {
    1 + len.saturating_sub(176).div_ceil(184)
}

/// One TS packet of stream `pid`'s PES and how much of `rest` it carries: the first with
/// payload_unit_start and the PCR (with the random access flag when `.1`), the last filled out
/// with adaptation-field stuffing.
fn pes_packet(
    pid: u16,
    start: bool,
    cc: u8,
    pcr: Option<(u64, bool)>,
    rest: &[u8],
) -> (Vec<u8>, usize) {
    // The adaptation field's bytes after its length byte.
    let mut field = Vec::new();
    if let Some((base, key)) = pcr {
        // A PCR (a 33-bit base, 6 reserved bits, a 9-bit extension of 0).
        field.extend([
            if key { 0x50 } else { 0x10 },
            (base >> 25) as u8,
            (base >> 17) as u8,
            (base >> 9) as u8,
            (base >> 1) as u8,
            (((base & 1) as u8) << 7) | 0x7e,
            0,
        ]);
    }
    let mut has_field = pcr.is_some();
    let room = if has_field { 183 - field.len() } else { 184 };
    let used = rest.len().min(room);
    let mut short = room - used;
    if short > 0 && !has_field {
        // A new field: its length byte takes one, a flags byte the next.
        has_field = true;
        short -= 1;
        if short > 0 {
            field.push(0x00);
            short -= 1;
        }
    }
    field.extend(std::iter::repeat_n(0xff, short));
    let control = if has_field { 0x30 } else { 0x10 };
    let mut p = vec![
        0x47,
        (u8::from(start) << 6) | (pid >> 8) as u8,
        pid as u8,
        control | cc,
    ];
    if has_field {
        p.push(field.len() as u8);
        p.extend(field);
    }
    p.extend(&rest[..used]);
    debug_assert_eq!(p.len(), TS_PACKET);
    (p, used)
}

/// A packet holding one PSI section (a pointer field of 0, then 0xff to the end).
fn psi_packet(pid: u16, cc: u8, section: &[u8]) -> Vec<u8> {
    let mut p = vec![0x47, 0x40 | (pid >> 8) as u8, pid as u8, 0x10 | cc, 0];
    p.extend(section);
    p.resize(TS_PACKET, 0xff);
    p
}

/// A long-form section: `table_id`, its length, `body` and its CRC.
fn section(table_id: u8, body: &[u8]) -> Vec<u8> {
    let len = body.len() + 4;
    let mut s = vec![table_id, 0xb0 | (len >> 8) as u8, len as u8];
    s.extend(body);
    let crc = crc32_mpeg(&s);
    s.extend(crc.to_be_bytes());
    s
}

/// The PAT: program 1, its PMT at `PMT_PID`.
fn pat() -> Vec<u8> {
    let pmt = [0xe0 | (PMT_PID >> 8) as u8, PMT_PID as u8];
    section(0x00, &[0, 1, 0xc1, 0, 0, 0, 1, pmt[0], pmt[1]])
}

/// The PMT: one stream of type `kind` (0x0f AAC in ADTS, 0x1b H.264) at `pid`, which carries
/// the PCR too.
fn pmt(kind: u8, pid: u16) -> Vec<u8> {
    let (hi, lo) = (0xe0 | (pid >> 8) as u8, pid as u8);
    section(
        0x02,
        &[0, 1, 0xc1, 0, 0, hi, lo, 0xf0, 0, kind, hi, lo, 0xf0, 0],
    )
}

/// An ADTS header (MPEG-4 AAC-LC, 32 kHz, mono, no CRC) for a frame of `len` bytes.
fn adts_header(len: usize) -> [u8; 7] {
    const RATE_INDEX: u8 = 5; // 32 kHz
    [
        0xff,
        0xf1,
        (1 << 6) | (RATE_INDEX << 2),
        (1 << 6) | ((len >> 11) & 3) as u8,
        (len >> 3) as u8,
        (((len & 7) as u8) << 5) | 0x1f,
        0xfc,
    ]
}

/// A PES timestamp: 4 bits of `prefix`, then the 33 bits of `ts` with their marker bits.
fn timestamp(prefix: u8, ts: u64) -> [u8; 5] {
    [
        (prefix << 4) | ((((ts >> 30) & 7) as u8) << 1) | 1,
        (ts >> 22) as u8,
        ((((ts >> 15) & 0x7f) as u8) << 1) | 1,
        (ts >> 7) as u8,
        (((ts & 0x7f) as u8) << 1) | 1,
    ]
}

/// MPEG-2's CRC-32 (polynomial 0x04c11db7, not reflected, from all ones, no final xor).
fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

const M3U8: &str = "application/vnd.apple.mpegurl";

/// `/hls/master.m3u8`.
pub(crate) async fn hls_master() -> Response {
    master("audio/index.m3u8")
}

/// `/hls/broken-audio.m3u8`: the same, its sound's pieces not MPEG-TS (seeded bytes): every
/// piece downloads, and ffmpeg can't read the sound to join it with the picture (a failed merge).
pub(crate) async fn hls_broken_master() -> Response {
    master("audio/broken.m3u8")
}

/// A master playlist of the video tracks, its sound at `audio` (relative to it).
fn master(audio: &str) -> Response {
    let mut s = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n\
         #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",LANGUAGE=\"en\",\
         DEFAULT=YES,AUTOSELECT=YES,URI=\"{audio}\"\n",
    );
    for t in &TRACKS {
        if let Some((w, h)) = t.video {
            s += &format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={w}x{h},\
                 CODECS=\"avc1.4d401f,mp4a.40.2\",AUDIO=\"aud\"\n{}/index.m3u8\n",
                t.bandwidth, t.name
            );
        }
    }
    text(M3U8, s)
}

/// `/hls/{track}/index.m3u8` (a media playlist) and `/hls/{track}/seg{i}.ts`.
pub(crate) async fn hls_file(Path((name, file)): Path<(String, String)>) -> Response {
    let Some((ti, t)) = track(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(prefix) = match file.as_str() {
        "index.m3u8" => Some("seg"),
        "broken.m3u8" => Some("broken"),
        _ => None,
    } {
        let mut s = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{SEGMENT_SECS}\n\
             #EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n"
        );
        for i in 0..SEGMENTS {
            s += &format!("#EXTINF:{SEGMENT_SECS}.000,\n{prefix}{i}.ts\n");
        }
        s += "#EXT-X-ENDLIST\n";
        return text(M3U8, s);
    }
    // #769: the same pieces encrypted with AES-128 (`enc.m3u8`): the first with an IV of its
    // own, the others with their sequence number's, all with `key.bin`.
    if file == "enc.m3u8" {
        let mut s = format!(
            "#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:{SEGMENT_SECS}
             #EXT-X-MEDIA-SEQUENCE:{ENC_SEQUENCE}
#EXT-X-PLAYLIST-TYPE:VOD
"
        );
        for i in 0..SEGMENTS {
            if i == 0 {
                s += "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x000000000000000000000000000007E9
";
            } else if i == 1 {
                s += "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"
";
            }
            s += &format!(
                "#EXTINF:{SEGMENT_SECS}.000,
enc{i}.ts
"
            );
        }
        s += "#EXT-X-ENDLIST
";
        return text(M3U8, s);
    }
    if file == "key.bin" {
        return bytes("application/octet-stream", ENC_KEY.to_vec());
    }
    if let Some(i) = numbered(&file, "enc", ".ts").filter(|i| *i < SEGMENTS) {
        let iv = if i == 0 {
            enc_iv(0x7E9)
        } else {
            enc_iv(ENC_SEQUENCE + u64::from(i))
        };
        return bytes(
            "video/mp2t",
            encrypt(&ts_segment(ti, i, t.segment_len), &iv),
        );
    }
    // Named as pieces, but no MPEG-TS: `broken.m3u8`'s.
    if let Some(i) = numbered(&file, "broken", ".ts").filter(|i| *i < SEGMENTS) {
        return bytes("video/mp2t", segment(ti, i, t.segment_len));
    }
    match numbered(&file, "seg", ".ts") {
        Some(i) if i < SEGMENTS => bytes("video/mp2t", ts_segment(ti, i, t.segment_len)),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// #900: the HLS stream again, served only as the browser asks for it from a page of `site` (a
/// host and port): every request, the playlists' and each segment's, must come with that page's
/// `Origin` and a `Referer` on it, as a site's video server checks (a player on another site's
/// page, a CDN that serves its own site's player only). Anything else gets 403. Its answers
/// allow that origin to read them (CORS), as the browser's player on that page does.
fn guarded(site: &str, headers: &HeaderMap, resp: Response) -> Response {
    let want = format!("http://{site}");
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok());
    let allowed = origin == Some(want.as_str())
        && referer.is_some_and(|r| r.starts_with(&format!("{want}/")));
    let mut resp = if allowed {
        resp
    } else {
        (
            StatusCode::FORBIDDEN,
            format!("the Origin and Referer of {want} are required\n"),
        )
            .into_response()
    };
    if let Ok(v) = want.parse() {
        resp.headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    resp
}

/// `/hlsg/{site}/master.m3u8` (#900): `/hls/master.m3u8` for pages of `site` only ([`guarded`]).
pub(crate) async fn hlsg_master(Path(site): Path<String>, headers: HeaderMap) -> Response {
    guarded(&site, &headers, hls_master().await)
}

/// `/hlsg/{site}/{track}/{file}` (#900): `/hls/{track}/{file}` for pages of `site` only.
pub(crate) async fn hlsg_file(
    Path((site, name, file)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    guarded(&site, &headers, hls_file(Path((name, file))).await)
}

/// The encrypted pieces' key (`/hls/{track}/key.bin`), their playlist's first sequence number.
const ENC_KEY: [u8; 16] = *b"mdm-lab-key-0769";
const ENC_SEQUENCE: u64 = 100;

/// An IV of `n`, right-aligned in 16 bytes (as a playlist's `IV=0x…`, and a piece's sequence
/// number when it has none).
fn enc_iv(n: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&n.to_be_bytes());
    iv
}

/// `data` encrypted with AES-128-CBC and PKCS#7 padding, as an HLS server sends a piece.
fn encrypt(data: &[u8], iv: &[u8; 16]) -> Vec<u8> {
    use aws_lc_rs::cipher::{
        AES_128, EncryptionContext, PaddedBlockEncryptingKey, UnboundCipherKey,
    };
    let key = PaddedBlockEncryptingKey::cbc_pkcs7(
        UnboundCipherKey::new(&AES_128, &ENC_KEY).expect("a 16-byte key"),
    )
    .expect("CBC");
    let mut out = data.to_vec();
    key.less_safe_encrypt(&mut out, EncryptionContext::Iv128((*iv).into()))
        .expect("encrypted");
    out
}

/// `N` of `{prefix}N{suffix}`.
fn numbered(file: &str, prefix: &str, suffix: &str) -> Option<u32> {
    file.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

/// `/dash/manifest.mpd`.
pub(crate) async fn dash_manifest() -> Response {
    text("application/dash+xml", mpd(false))
}

/// `/dash/protected.mpd`: the same tracks, each marked as encrypted (CENC, with Widevine's key
/// system), as a DRM-protected stream's manifest is. Its segments aren't encrypted: no client
/// should get that far.
pub(crate) async fn dash_protected() -> Response {
    text("application/dash+xml", mpd(true))
}

/// Widevine's DASH key system.
const WIDEVINE: &str = "urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed";

fn mpd(protected: bool) -> String {
    let protection = if protected {
        format!(
            "<ContentProtection schemeIdUri=\"urn:mpeg:dash:mp4protection:2011\" value=\"cenc\"/>\
             <ContentProtection schemeIdUri=\"{WIDEVINE}\"/>"
        )
    } else {
        String::new()
    };
    let total = SEGMENTS * SEGMENT_SECS;
    let template = format!(
        "<SegmentTemplate initialization=\"$RepresentationID$/init.mp4\" \
         media=\"$RepresentationID$/seg$Number$.m4s\" startNumber=\"1\" \
         duration=\"{SEGMENT_SECS}\" timescale=\"1\"/>"
    );
    let mut video = String::new();
    let mut audio = String::new();
    for t in &TRACKS {
        match t.video {
            Some((w, h)) => {
                video += &format!(
                    "<Representation id=\"{}\" bandwidth=\"{}\" width=\"{w}\" height=\"{h}\" \
                     codecs=\"avc1.4d401f\"/>",
                    t.name, t.bandwidth
                )
            }
            None => {
                audio += &format!(
                    "<Representation id=\"{}\" bandwidth=\"{}\" codecs=\"mp4a.40.2\" \
                     audioSamplingRate=\"48000\"/>",
                    t.name, t.bandwidth
                )
            }
        }
    }
    let s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" type=\"static\" \
         profiles=\"urn:mpeg:dash:profile:isoff-live:2011\" minBufferTime=\"PT2S\" \
         mediaPresentationDuration=\"PT{total}S\">\n\
         <Period id=\"0\" start=\"PT0S\">\n\
         <AdaptationSet id=\"0\" contentType=\"video\" mimeType=\"video/mp4\" \
         segmentAlignment=\"true\">{protection}{template}{video}</AdaptationSet>\n\
         <AdaptationSet id=\"1\" contentType=\"audio\" mimeType=\"audio/mp4\" lang=\"en\" \
         segmentAlignment=\"true\">{protection}{template}{audio}</AdaptationSet>\n\
         </Period>\n</MPD>\n"
    );
    s
}

/// #171: the same HLS stream with no `.m3u8` in any address and served as `text/plain`, as some
/// CDNs and players do: `/hls2/playlist?id=ID` (the master), `/hls2/playlist?id=ID&track=T` (a
/// media playlist) and `/hls2/seg?id=ID&track=T&i=N` (`application/octet-stream`).
pub(crate) async fn hls2_playlist(Query(q): Query<HashMap<String, String>>) -> Response {
    let id = utf8_percent_encode(q.get("id").map_or("lab", String::as_str), NON_ALPHANUMERIC)
        .to_string();
    let Some(name) = q.get("track") else {
        let mut s = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",LANGUAGE=\"en\",\
             DEFAULT=YES,AUTOSELECT=YES,URI=\"playlist?id={id}&track=audio\"\n"
        );
        for t in &TRACKS {
            if let Some((w, h)) = t.video {
                s += &format!(
                    "#EXT-X-STREAM-INF:BANDWIDTH={},RESOLUTION={w}x{h},\
                     CODECS=\"avc1.4d401f,mp4a.40.2\",AUDIO=\"aud\"\nplaylist?id={id}&track={}\n",
                    t.bandwidth, t.name
                );
            }
        }
        return text(PLAIN, s);
    };
    if track(name).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut s = format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{SEGMENT_SECS}\n\
         #EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n"
    );
    for i in 0..SEGMENTS {
        s += &format!("#EXTINF:{SEGMENT_SECS}.000,\nseg?id={id}&track={name}&i={i}\n");
    }
    s += "#EXT-X-ENDLIST\n";
    text(PLAIN, s)
}

const PLAIN: &str = "text/plain; charset=utf-8";

/// `/hls2/seg?track=T&i=N`: segment N of track T, as `application/octet-stream`.
pub(crate) async fn hls2_segment(Query(q): Query<HashMap<String, String>>) -> Response {
    let found = q.get("track").and_then(|n| track(n));
    let i = q.get("i").and_then(|i| i.parse::<u32>().ok());
    match (found, i) {
        (Some((ti, t)), Some(i)) if i < SEGMENTS => {
            bytes("application/octet-stream", ts_segment(ti, i, t.segment_len))
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `/dash2/manifest?id=ID`: the DASH manifest with no `.mpd` in its address, served as
/// `application/octet-stream`; its segments are `/dash2/{rep}/init.mp4` and `seg{n}.m4s`.
pub(crate) async fn dash2_manifest() -> Response {
    text("application/octet-stream", mpd(false))
}

/// `/hls2/config?id=ID`: a player's settings as `text/plain`, not a manifest (the pages' decoy:
/// the extension may read it and must never list it).
pub(crate) async fn hls2_config() -> Response {
    text(PLAIN, "{\"autoplay\":false,\"volume\":0.8}\n".to_owned())
}

/// `/dash/{rep}/init.mp4` and `/dash/{rep}/seg{n}.m4s` (n from 1).
pub(crate) async fn dash_file(Path((name, file)): Path<(String, String)>) -> Response {
    let Some((ti, t)) = track(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ty = if t.video.is_some() {
        "video/mp4"
    } else {
        "audio/mp4"
    };
    if file == "init.mp4" {
        let mut buf = segment(ti, 999, 1024);
        let ftyp = b"\0\0\0\x18ftypiso6\0\0\0\0iso6dash";
        buf[..ftyp.len()].copy_from_slice(ftyp);
        return bytes(ty, buf);
    }
    match numbered(&file, "seg", ".m4s") {
        Some(n) if (1..=SEGMENTS).contains(&n) => bytes(ty, segment(ti, n, t.segment_len)),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC-32/MPEG-2's check value.
    #[test]
    fn sections_carry_mpeg_crcs() {
        assert_eq!(crc32_mpeg(b"123456789"), 0x0376_e6e7);
    }

    fn pid_of(p: &[u8]) -> u16 {
        (u16::from(p[1] & 0x1f) << 8) | u16::from(p[2])
    }

    #[test]
    fn segments_are_whole_packets_with_their_stream_first() {
        for (ti, t) in TRACKS.iter().enumerate() {
            for i in 0..SEGMENTS {
                let s = ts_segment(ti, i, t.segment_len);
                assert_eq!(s.len(), t.segment_len);
                assert!(s.chunks(TS_PACKET).all(|p| p[0] == 0x47));
                let pids: Vec<u16> = s.chunks(TS_PACKET).map(pid_of).collect();
                assert_eq!(&pids[..2], [0, PMT_PID]);
                let (pid, kind) = match t.video {
                    Some(_) => (VIDEO_PID, 0x1b),
                    None => (AUDIO_PID, 0x0f),
                };
                // The PMT names the track's one stream.
                assert_eq!(s[TS_PACKET + 5 + 12], kind, "{}", t.name);
                let stream = pids.iter().filter(|p| **p == pid).count();
                assert!(pids[2..2 + stream].iter().all(|p| *p == pid));
                assert!(pids[2 + stream..].iter().all(|p| *p == NULL_PID));
                // The PES starts in the stream's first packet, after its adaptation field
                // (a PCR, and the random access flag).
                let first = &s[2 * TS_PACKET..3 * TS_PACKET];
                assert_eq!(first[1] & 0x40, 0x40);
                assert_eq!(first[5], 0x50);
                let payload = &first[5 + first[4] as usize..];
                if t.video.is_none() {
                    assert_eq!(&payload[..4], [0, 0, 1, 0xc0]);
                    // ADTS frames back to back.
                    assert_eq!(&payload[14..16], [0xff, 0xf1]);
                    assert_eq!(&payload[14 + 11..14 + 13], [0xff, 0xf1]);
                } else {
                    // A picture: its access unit delimiter, then the SPS (an IDR frame).
                    assert_eq!(&payload[..4], [0, 0, 1, 0xe0]);
                    assert_eq!(&payload[14..19], [0, 0, 0, 1, 0x09]);
                    assert_eq!(&payload[20..25], [0, 0, 0, 1, 0x67]);
                    // A PES a frame: SEGMENT_SECS × FPS of them.
                    let starts = s
                        .chunks(TS_PACKET)
                        .filter(|p| pid_of(p) == pid && p[1] & 0x40 != 0)
                        .count();
                    assert_eq!(starts, (SEGMENT_SECS * FPS) as usize);
                }
            }
        }
    }
}
