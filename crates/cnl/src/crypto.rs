//! What Click'n'Load 2 posts is opened with: AES-128-CBC, Base64, a link a line.

/// AES-128 in CBC mode, the padding left to the caller (Click'n'Load pads with zeros): `data`'s
/// length a multiple of 16, else None.
pub(crate) fn aes_cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Option<Vec<u8>> {
    use aes::cipher::{BlockCipherDecrypt, KeyInit};
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return None;
    }
    let cipher = aes::Aes128::new(key.into());
    let mut prev = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.as_chunks::<16>().0 {
        let mut block = aes::Block::from(*chunk);
        cipher.decrypt_block(&mut block);
        out.extend(block.iter().zip(prev).map(|(b, p)| b ^ p));
        prev = *chunk;
    }
    Some(out)
}

/// The other way, for the tests' own posts.
#[cfg(test)]
pub(crate) fn aes_cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
    use aes::cipher::{BlockCipherEncrypt, KeyInit};
    let cipher = aes::Aes128::new(key.into());
    let mut padded = data.to_vec();
    padded.resize(data.len().div_ceil(16) * 16, 0);
    let mut prev = *iv;
    let mut out = Vec::with_capacity(padded.len());
    for chunk in padded.as_chunks::<16>().0 {
        let mut block = aes::Block::default();
        for (b, (c, p)) in block.iter_mut().zip(chunk.iter().zip(prev)) {
            *b = c ^ p;
        }
        cipher.encrypt_block(&mut block);
        prev.copy_from_slice(&block);
        out.extend_from_slice(&block);
    }
    out
}

/// Base64 as the sites write it: standard alphabet, padding optional, line breaks and spaces
/// ignored.
pub(crate) fn base64(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(clean.trim_end_matches('='))
        .ok()
}

/// The links in a decrypted text: one a line, http(s) and ftp only, each once, NUL padding
/// and blanks dropped. A link to this computer or its network ([`internal`]) is left out: a web
/// page sent these, and such an address is never one of a link site's files.
pub(crate) fn lines(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut inside = 0;
    let links: Vec<String> = text
        .split(['\r', '\n'])
        .map(|l| l.trim_matches(|c: char| c == '\0' || c.is_whitespace()))
        .filter(|l| {
            url::Url::parse(l).is_ok_and(|u| {
                let web = matches!(u.scheme(), "http" | "https" | "ftp");
                let internal = web && internal(&u);
                inside += usize::from(internal);
                web && !internal
            })
        })
        .filter(|l| seen.insert(l.to_string()))
        .map(str::to_owned)
        .collect();
    if inside > 0 {
        tracing::info!("Click'n'Load: {inside} link(s) to this computer or its network left out");
    }
    links
}

/// Whether an address is on this computer or its own network, by its host as written: a
/// loopback, private, link-local, shared (100.64/10), unspecified, broadcast or multicast IP
/// address (IPv4, IPv6, or IPv4 inside IPv6), `localhost`, a name without a dot, or a name under
/// `.localhost`, `.local`, `.internal`, `.lan`, `.home.arpa` or `.intranet`. The `url` crate
/// reads every spelling of an IPv4 address (`2130706433`, `0x7f.1`) as the address it is.
pub(crate) fn internal(u: &url::Url) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};
    fn v4(a: Ipv4Addr) -> bool {
        let o = a.octets();
        a.is_loopback()
            || a.is_private()
            || a.is_link_local()
            || a.is_unspecified()
            || a.is_broadcast()
            || a.is_multicast()
            || o[0] == 0
            || (o[0] == 100 && o[1] & 0xc0 == 64)
            || (o[0] == 192 && o[1] == 0 && o[2] == 0)
            || (o[0] == 198 && o[1] & 0xfe == 18)
            || o[0] >= 240
    }
    fn v6(a: Ipv6Addr) -> bool {
        let s = a.segments();
        // IPv4 inside IPv6: mapped (::ffff:a.b.c.d), compatible (::a.b.c.d), NAT64 (64:ff9b::).
        let embedded = (s[..5] == [0; 5] && (s[5] == 0xffff || s[5] == 0))
            || (s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4]);
        if embedded && !a.is_unspecified() && !a.is_loopback() {
            return v4(Ipv4Addr::new(
                (s[6] >> 8) as u8,
                s[6] as u8,
                (s[7] >> 8) as u8,
                s[7] as u8,
            ));
        }
        a.is_loopback()
            || a.is_unspecified()
            || a.is_multicast()
            || s[0] & 0xffc0 == 0xfe80
            || s[0] & 0xfe00 == 0xfc00
    }
    match u.host() {
        Some(url::Host::Ipv4(a)) => v4(a),
        Some(url::Host::Ipv6(a)) => v6(a),
        Some(url::Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !d.contains('.')
                || [
                    ".localhost",
                    ".local",
                    ".internal",
                    ".lan",
                    ".home.arpa",
                    ".intranet",
                ]
                .iter()
                .any(|end| d.ends_with(end))
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbc_goes_both_ways_and_refuses_partial_blocks() {
        let (key, iv) = (*b"0123456789abcdef", *b"fedcba9876543210");
        let text = b"a text longer than one block, zero padded";
        let sealed = aes_cbc_encrypt(&key, &iv, text);
        assert_eq!(sealed.len() % 16, 0);
        let open = aes_cbc_decrypt(&key, &iv, &sealed).unwrap();
        assert_eq!(&open[..text.len()], text);
        assert!(open[text.len()..].iter().all(|b| *b == 0));
        assert_eq!(aes_cbc_decrypt(&key, &iv, &sealed[1..]), None);
    }

    #[test]
    fn links_one_a_line_each_once() {
        let t = "http://a.example/1\r\n\nhttps://b.example/2\nnot a link\nhttp://a.example/1\0\0";
        assert_eq!(lines(t), vec!["http://a.example/1", "https://b.example/2"]);
        assert_eq!(base64("aGVs\nbG8"), Some(b"hello".to_vec()));
    }

    #[test]
    fn links_to_this_computer_or_its_network_are_left_out() {
        let inside = [
            "http://127.0.0.1/admin",
            "http://127.1/",
            "http://2130706433/",
            "http://0x7f.0.0.1/",
            "http://localhost:8080/x",
            "http://LOCALHOST./x",
            "http://printer/",
            "http://nas.local/share",
            "http://router.lan/",
            "http://db.internal/",
            "http://10.0.0.5/a",
            "http://172.16.4.1/a",
            "http://172.31.255.255/a",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://0.0.0.0/",
            "http://255.255.255.255/",
            "ftp://192.168.0.2/file",
            "http://[::1]/",
            "http://[::]/",
            "http://[fe80::1]/",
            "http://[fd12:3456::1]/",
            "http://[::ffff:192.168.1.1]/",
            "http://[::ffff:7f00:1]/",
            "http://[64:ff9b::a00:1]/",
        ];
        for l in inside {
            assert!(internal(&url::Url::parse(l).unwrap()), "{l}");
        }
        let outside = [
            "https://a.example/1.zip",
            "http://8.8.8.8/x",
            "http://172.32.0.1/x",
            "http://100.128.0.1/x",
            "http://[2001:4860:4860::8888]/x",
            "http://[::ffff:8.8.8.8]/x",
            "ftp://files.example.org/a",
            "https://my.locally.example/x",
        ];
        for l in outside {
            assert!(!internal(&url::Url::parse(l).unwrap()), "{l}");
        }
        let t =
            "https://a.example/1\nhttp://192.168.1.1/cgi\nhttp://localhost/x\nhttps://b.example/2";
        assert_eq!(lines(t), vec!["https://a.example/1", "https://b.example/2"]);
    }
}
