//! Local inference using Google's MediaPipe Tasks runtime through its safe Rust API.
//! RGB bytes are an inference input, never an image-editing pixel representation.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Detect one face in a bounded, tightly packed sRGB inference thumbnail.
/// Coordinates are normalized to this thumbnail. No image is sent over the network.
#[cfg(not(target_arch = "wasm32"))]
pub fn face_landmarks(model: &std::path::Path, width: u32, height: u32, rgb: &[u8]) -> Result<Vec<Vec<[f64; 3]>>, String> {
    use mediapipe::{FaceLandmarker, Image, ModelSource, Size};
    let count = u64::from(width).saturating_mul(u64::from(height));
    if width == 0 || height == 0 || count > 2_560_000 || count.saturating_mul(3) != rgb.len() as u64 {
        return Err("Invalid face-detection thumbnail (maximum 1600 x 1600 pixels)".into());
    }
    if !model.is_file() {
        return Err("Face detection model is missing. Run scripts/setup-face-model.ps1 once.".into());
    }
    let image = Image::from_rgb(Size { width, height }, rgb).map_err(|e| format!("MediaPipe could not read the photo: {e}"))?;
    let mut detector = FaceLandmarker::builder(ModelSource::path(model))
        .num_faces(std::num::NonZeroU32::new(1).ok_or("Invalid face limit")?)
        .build()
        .map_err(|e| format!("MediaPipe could not start: {e}"))?;
    let result = detector.detect(&image).map_err(|e| format!("Face detection failed: {e}"))?;
    let faces: Vec<_> = result
        .landmarks
        .into_iter()
        .map(|face| face.into_iter().map(|p| [f64::from(p.point.x()), f64::from(p.point.y()), f64::from(p.point.z())]).collect::<Vec<_>>())
        .collect();
    if faces.iter().any(|f| f.len() != 478 || f.iter().flatten().any(|v| !v.is_finite() || v.abs() > 2.0)) {
        return Err("MediaPipe returned invalid face landmarks".into());
    }
    Ok(faces)
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn rejects_invalid_thumbnail_before_loading_native_runtime() {
        let path = std::path::Path::new("missing.task");
        assert!(super::face_landmarks(path, 0, 1, &[]).is_err());
        assert!(super::face_landmarks(path, u32::MAX, u32::MAX, &[]).is_err());
        assert!(super::face_landmarks(path, 16, 16, &[0; 3]).is_err());
        assert!(super::face_landmarks(path, 1, 1, &[0; 3]).is_err());
    }
}
