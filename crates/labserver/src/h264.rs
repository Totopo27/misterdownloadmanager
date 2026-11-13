//! A real H.264 picture stream for the lab's HLS video tracks (ITU-T H.264): a grey picture,
//! one IDR frame then frames that skip every macroblock, so ffmpeg finds and copies a video
//! stream as it does a site's. yt-dlp merges a video track with `-map 0:v:0`, which fails on a
//! track with no picture ("Stream map '' matches no streams", "Error opening output files:
//! Invalid argument"): the lab's video tracks once carried only sound.
//!
//! Main profile (the playlists' `avc1.4d401f`), CAVLC. Each macroblock of the IDR frame is
//! `I_16x16_2_0_0` (DC prediction, nothing coded), which decodes to mid grey; a later frame is
//! one P slice whose skip run covers the whole picture.

/// Bits written most significant first, with Exp-Golomb codes (H.264 9.1).
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u8,
    n: u8,
}

impl Bits {
    fn bit(&mut self, b: bool) {
        self.acc = (self.acc << 1) | u8::from(b);
        self.n += 1;
        if self.n == 8 {
            self.out.push(self.acc);
            self.acc = 0;
            self.n = 0;
        }
    }

    /// `u(n)`: the low `n` bits of `v`.
    fn u(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 == 1);
        }
    }

    /// `ue(v)`: unsigned Exp-Golomb.
    fn ue(&mut self, v: u32) {
        let x = v + 1;
        let len = 32 - x.leading_zeros();
        self.u(len - 1, 0);
        self.u(len, x);
    }

    /// `rbsp_trailing_bits`: a one, then zeros to the byte.
    fn finish(mut self) -> Vec<u8> {
        self.bit(true);
        while self.n != 0 {
            self.bit(false);
        }
        self.out
    }
}

/// A NAL unit with its 4-byte start code: `header`, then the RBSP with emulation prevention
/// (a 0x03 after two zeros when the next byte is 0 to 3).
fn nal(header: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, header];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros == 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// `log2_max_frame_num`: 256 frames, more than a segment holds (each starts with an IDR).
const FRAME_NUM_BITS: u32 = 8;

/// The picture's size in macroblocks.
fn mbs(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(16), height.div_ceil(16))
}

/// The sequence parameter set: Main profile, level 3.1, `fps` frames a second.
fn sps(width: u32, height: u32, fps: u32) -> Vec<u8> {
    let (w, h) = mbs(width, height);
    let mut b = Bits::default();
    b.u(8, 77); // profile_idc: Main
    b.u(8, 0x40); // constraint_set1_flag
    b.u(8, 31); // level_idc
    b.ue(0); // seq_parameter_set_id
    b.ue(FRAME_NUM_BITS - 4); // log2_max_frame_num_minus4
    b.ue(2); // pic_order_cnt_type: from frame_num (no B frames)
    b.ue(1); // max_num_ref_frames
    b.u(1, 0); // gaps_in_frame_num_value_allowed_flag
    b.ue(w - 1); // pic_width_in_mbs_minus1
    b.ue(h - 1); // pic_height_in_map_units_minus1
    b.u(1, 1); // frame_mbs_only_flag
    b.u(1, 1); // direct_8x8_inference_flag
    let (crop_right, crop_bottom) = ((w * 16 - width) / 2, (h * 16 - height) / 2);
    let crop = crop_right > 0 || crop_bottom > 0;
    b.u(1, u32::from(crop)); // frame_cropping_flag
    if crop {
        b.ue(0);
        b.ue(crop_right);
        b.ue(0);
        b.ue(crop_bottom);
    }
    b.u(1, 1); // vui_parameters_present_flag
    b.u(4, 0); // aspect ratio, overscan, video signal type, chroma location: none
    b.u(1, 1); // timing_info_present_flag
    b.u(32, 1); // num_units_in_tick
    b.u(32, fps * 2); // time_scale (two ticks a frame)
    b.u(1, 1); // fixed_frame_rate_flag
    b.u(4, 0); // no HRD, no pic_struct, no bitstream restriction
    nal(0x67, &b.finish())
}

/// The picture parameter set: CAVLC, one reference, QP 26.
fn pps() -> Vec<u8> {
    let mut b = Bits::default();
    b.ue(0); // pic_parameter_set_id
    b.ue(0); // seq_parameter_set_id
    b.u(1, 0); // entropy_coding_mode_flag: CAVLC
    b.u(1, 0); // bottom_field_pic_order_in_frame_present_flag
    b.ue(0); // num_slice_groups_minus1
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.u(1, 0); // weighted_pred_flag
    b.u(2, 0); // weighted_bipred_idc
    b.ue(0); // pic_init_qp_minus26: se(0)
    b.ue(0); // pic_init_qs_minus26: se(0)
    b.ue(0); // chroma_qp_index_offset: se(0)
    b.u(3, 0); // no deblocking control, constrained intra or redundant pictures
    nal(0x68, &b.finish())
}

/// An access unit delimiter (any picture type), as HLS segments carry before each frame.
const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xf0];

/// Frame `n` (from 0) of a segment: an access unit delimiter, then for the first the SPS, the
/// PPS and an IDR slice (`idr_id` 1 or 2, distinct from the segment before's), for the others a
/// P slice that skips every macroblock.
pub fn frame(width: u32, height: u32, fps: u32, idr_id: u32, n: u32) -> Vec<u8> {
    let (w, h) = mbs(width, height);
    let mut out = AUD.to_vec();
    let mut b = Bits::default();
    b.ue(0); // first_mb_in_slice
    if n == 0 {
        out.extend(sps(width, height, fps));
        out.extend(pps());
        b.ue(7); // slice_type: I, every slice
        b.ue(0); // pic_parameter_set_id
        b.u(FRAME_NUM_BITS, 0); // frame_num
        b.ue(idr_id); // idr_pic_id
        b.u(2, 0); // no_output_of_prior_pics_flag, long_term_reference_flag
        b.ue(0); // slice_qp_delta: se(0)
        for _ in 0..w * h {
            // mb_type I_16x16_2_0_0, intra_chroma_pred_mode DC, mb_qp_delta 0, then the luma
            // DC block's coeff_token for no coefficients (nC 0): 00100 1 1 1.
            b.u(8, 0b0010_0111);
        }
        out.extend(nal(0x65, &b.finish()));
    } else {
        b.ue(5); // slice_type: P, every slice
        b.ue(0); // pic_parameter_set_id
        b.u(FRAME_NUM_BITS, n); // frame_num: every frame is a reference
        b.u(1, 0); // num_ref_idx_active_override_flag
        b.u(1, 0); // ref_pic_list_modification_flag_l0
        b.u(1, 0); // adaptive_ref_pic_marking_mode_flag
        b.ue(0); // slice_qp_delta: se(0)
        b.ue(w * h); // mb_skip_run: the whole picture
        out.extend(nal(0x41, &b.finish()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        let mut b = Bits::default();
        for v in [0, 1, 2, 3, 7] {
            b.ue(v);
        }
        // 1 010 011 00100 0001000, then the trailing one.
        assert_eq!(b.finish(), [0b1010_0110, 0b0100_0001, 0b0001_0000]);
    }

    #[test]
    fn emulation_prevention() {
        assert_eq!(
            nal(0x67, &[0, 0, 1, 0, 0, 0, 0, 0, 4]),
            [0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0, 0, 3, 0, 4]
        );
    }

    /// An IDR frame: delimiter, SPS, PPS, the slice (one byte a macroblock); a P frame: the
    /// delimiter and its slice.
    #[test]
    fn frames() {
        let idr = frame(640, 360, 25, 1, 0);
        assert_eq!(&idr[..6], AUD);
        assert_eq!(&idr[6..13], [0, 0, 0, 1, 0x67, 77, 0x40]);
        let types: Vec<u8> = idr
            .windows(4)
            .filter(|w| w[..3] == [0, 0, 1])
            .map(|w| w[3] & 0x1f)
            .collect();
        assert_eq!(types, [9, 7, 8, 5]);
        // The slice: a byte a macroblock (40 × 23), after its header.
        let slice = idr.len()
            - idr
                .windows(5)
                .rposition(|w| w == [0, 0, 0, 1, 0x65])
                .unwrap();
        assert!((40 * 23..40 * 23 + 10).contains(&slice), "{slice}");
        let p = frame(640, 360, 25, 1, 7);
        assert_eq!(&p[..6], AUD);
        assert_eq!(p[10], 0x41);
        assert!(p.len() < 20);
    }
}
