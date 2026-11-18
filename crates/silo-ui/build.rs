fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.png");

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

#[cfg(windows)]
fn embed_windows_icon() {
    use std::fs::File;
    use std::io::BufWriter;
    use std::path::PathBuf;

    use image::ExtendedColorType;
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::imageops::FilterType;

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let ico_path = out_dir.join("silo.ico");

    let source = image::open("../../assets/icon.png")
        .expect("Failed to open ../../assets/icon.png for Windows executable icon");

    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut frames = Vec::with_capacity(sizes.len());
    for &size in &sizes {
        let resized = source.resize_exact(size, size, FilterType::Lanczos3).to_rgba8();
        let frame = IcoFrame::as_png(resized.as_raw(), size, size, ExtendedColorType::Rgba8)
            .expect("Failed to encode ICO frame");
        frames.push(frame);
    }

    let file = File::create(&ico_path).expect("Failed to create silo.ico in OUT_DIR");
    IcoEncoder::new(BufWriter::new(file))
        .encode_images(&frames)
        .expect("Failed to write multi-resolution silo.ico");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(
        ico_path
            .to_str()
            .expect("OUT_DIR silo.ico path is not valid UTF-8"),
    );
    res.compile()
        .expect("Failed to compile Windows icon resource into silo.exe");
}
