//! Local smoke test: cargo run -p photocraft-ml --example detect_faces -- model.task image.png
#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use mediapipe::{FaceLandmarker, Image, ModelSource};
    let args: Vec<_> = std::env::args().collect();
    let model = args.get(1).ok_or("Pass model.task and an image path")?;
    let image = Image::from_file(args.get(2).ok_or("Pass an image path")?)?;
    let mut detector = FaceLandmarker::builder(ModelSource::path(model)).build()?;
    let result = detector.detect(&image)?;
    println!("faces={} landmarks={}", result.landmarks.len(), result.landmarks.first().map_or(0, Vec::len));
    if args.iter().any(|a| a == "--landmarks") {
        let points: Vec<Vec<[f64; 2]>> =
            result.landmarks.iter().map(|face| face.iter().map(|p| [f64::from(p.point.x()), f64::from(p.point.y())]).collect()).collect();
        println!("{points:?}");
    }
    Ok(())
}
#[cfg(target_arch = "wasm32")]
fn main() {}
