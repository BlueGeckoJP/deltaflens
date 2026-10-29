//! Conversion of images with an embedded ICC profile to sRGB, the color space
//! they are shown in.

use eyre::{Result, bail};
use moxcms::{ColorProfile, DataColorSpace, Layout, TransformOptions, Xyzd};

/// How many pixels are converted at a time, so that a copy of the whole image
/// is not needed.
const CHUNK_PIXELS: usize = 64 * 1024;

/// Converts RGBA `pixels` from the color space of the `icc` profile to sRGB.
/// Pixels of an sRGB profile are left as they are.
///
/// # Errors
///
/// Returns an error if the profile cannot be read, or is for a color space
/// other than RGB or gray, such as CMYK.
pub(super) fn convert_to_srgb(pixels: &mut [u8], icc: &[u8]) -> Result<()> {
    let profile = ColorProfile::new_from_slice(icc)?;
    let srgb = ColorProfile::new_srgb();
    let options = TransformOptions::default();
    match profile.color_space {
        DataColorSpace::Rgb => {
            if is_srgb(&profile) {
                return Ok(());
            }
            let transform =
                profile.create_transform_8bit(Layout::Rgba, &srgb, Layout::Rgba, options)?;
            let mut source = vec![0; CHUNK_PIXELS * 4];
            for target in pixels.chunks_mut(CHUNK_PIXELS * 4) {
                let source = &mut source[..target.len()];
                source.copy_from_slice(target);
                transform.transform(source, target)?;
            }
        }
        // Gray pixels are in RGBA with equal color channels.
        DataColorSpace::Gray => {
            let transform =
                profile.create_transform_8bit(Layout::GrayAlpha, &srgb, Layout::Rgba, options)?;
            let mut source = Vec::with_capacity(CHUNK_PIXELS * 2);
            for target in pixels.chunks_mut(CHUNK_PIXELS * 4) {
                source.clear();
                source.extend(
                    target
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|&[v, _, _, a]| [v, a]),
                );
                transform.transform(&source, target)?;
            }
        }
        other => bail!("ICC profiles for {other:?} are not supported"),
    }
    Ok(())
}

/// Returns whether `profile` describes sRGB: the primaries, white point and
/// tone curves of sRGB, however the profile stores them.
fn is_srgb(profile: &ColorProfile) -> bool {
    // ICC profiles store numbers with 16 fractional bits, and programs adapt
    // the primaries to the D50 white point with slightly different matrices.
    const TOLERANCE: f64 = 2e-3;
    let close = |a: Xyzd, b: Xyzd| {
        (a.x - b.x).abs() < TOLERANCE
            && (a.y - b.y).abs() < TOLERANCE
            && (a.z - b.z).abs() < TOLERANCE
    };
    let srgb = ColorProfile::new_srgb();
    let Ok(srgb_curve) = srgb.build_8bit_lin_table(&srgb.red_trc) else {
        return false;
    };
    let same_curve = |trc| {
        profile.build_8bit_lin_table(trc).is_ok_and(|curve| {
            curve
                .iter()
                .zip(srgb_curve.iter())
                .all(|(a, b)| (a - b).abs() < 1e-3)
        })
    };
    profile.is_matrix_shaper()
        && close(profile.red_colorant, srgb.red_colorant)
        && close(profile.green_colorant, srgb.green_colorant)
        && close(profile.blue_colorant, srgb.blue_colorant)
        && same_curve(&profile.red_trc)
        && same_curve(&profile.green_trc)
        && same_curve(&profile.blue_trc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: &[u8], expected: &[u8]) {
        let close = actual
            .iter()
            .zip(expected)
            .all(|(&a, &e)| a.abs_diff(e) <= 3);
        assert!(close, "{actual:?} is not close to {expected:?}");
    }

    #[test]
    fn srgb_profile_is_recognized() {
        let icc = ColorProfile::new_srgb().encode().unwrap();
        assert!(is_srgb(&ColorProfile::new_from_slice(&icc).unwrap()));
    }

    #[test]
    fn other_profiles_are_not_srgb() {
        for profile in [
            ColorProfile::new_display_p3(),
            ColorProfile::new_adobe_rgb(),
            ColorProfile::new_bt2020(),
        ] {
            let icc = profile.encode().unwrap();
            assert!(!is_srgb(&ColorProfile::new_from_slice(&icc).unwrap()));
        }
    }

    #[test]
    fn srgb_pixels_are_left_alone() {
        let icc = ColorProfile::new_srgb().encode().unwrap();
        let mut pixels = vec![12, 34, 56, 78];
        convert_to_srgb(&mut pixels, &icc).unwrap();
        assert_eq!(pixels, [12, 34, 56, 78]);
    }

    #[test]
    fn display_p3_is_converted() {
        // The red of sRGB is about (234, 51, 35) in Display P3.
        let icc = ColorProfile::new_display_p3().encode().unwrap();
        let mut pixels = [[234, 51, 35, 255], [255, 255, 255, 128]].concat();
        convert_to_srgb(&mut pixels, &icc).unwrap();
        // Alpha is kept, and white stays white.
        assert_close(&pixels, &[255, 0, 0, 255, 255, 255, 255, 128]);
    }

    #[test]
    fn gray_profile_is_converted() {
        // Gamma 2.2 is close to the curve of sRGB: mid gray becomes 129.
        let icc = ColorProfile::new_gray_with_gamma(2.2).encode().unwrap();
        let mut pixels = [[0, 0, 0, 255], [128, 128, 128, 200], [255, 255, 255, 255]].concat();
        convert_to_srgb(&mut pixels, &icc).unwrap();
        assert_close(
            &pixels,
            &[0, 0, 0, 255, 129, 129, 129, 200, 255, 255, 255, 255],
        );
    }

    #[test]
    fn cmyk_profiles_are_rejected() {
        let mut profile = ColorProfile::new_srgb();
        profile.color_space = DataColorSpace::Cmyk;
        let icc = profile.encode().unwrap();
        assert!(convert_to_srgb(&mut [0, 0, 0, 255], &icc).is_err());
    }
}
