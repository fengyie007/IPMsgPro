//! Optional local-fixture verification, never copies private samples into git.
//! cargo run --offline -p ipmsg-core --example verify_image -- <RawLZW.bin> <sender.bmp>
use ipmsg_core::image::dib::decode_image;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("expected raw payload and original BMP paths".into());
    }
    let payload = std::fs::read(&args[0])?;
    let original = image::load_from_memory(&std::fs::read(&args[1])?)?.to_rgba8();
    let decoded = decode_image(&payload).map_err(std::io::Error::other)?;
    let image = image::load_from_memory(&decoded.png)?.to_rgba8();
    if original.dimensions() != image.dimensions() || original.as_raw() != image.as_raw() {
        return Err("decoded image differs from original BMP pixels".into());
    }
    let encoded =
        ipmsg_core::image::dib::encode_png_for_wire(&decoded.png).map_err(std::io::Error::other)?;
    let round = decode_image(&encoded).map_err(std::io::Error::other)?;
    let sent = image::load_from_memory(&round.png)?.to_rgba8();
    if sent != original {
        return Err("outbound encoding differs from original BMP pixels".into());
    }
    println!(
        "Verified {}x{}: receive and outbound round-trip pixels match original BMP",
        decoded.width, decoded.height
    );
    Ok(())
}
