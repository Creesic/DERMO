//! Generates macOS .iconset from a source image with squircle mask.
//! Run: cargo run --bin build_icon -- assets/Dermologo.jpg dist/icon.iconset

use image::{imageops, ImageFormat};
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: build_icon <input.jpg> <output.iconset>");
        std::process::exit(1);
    }
    let input = &args[1];
    let output = &args[2];

    let img = image::open(input).expect("Failed to open image");
    let rgba = img.to_rgba8();

    fs::create_dir_all(output).expect("Failed to create output dir");

    let sizes = [
        (16, "icon_16x16.png"),
        (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"),
        (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"),
        (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"),
        (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"),
        (1024, "icon_512x512@2x.png"),
    ];

    for (size, name) in sizes {
        let out_path = Path::new(output).join(name);
        let buf = make_squircle_icon(&rgba, size);
        let img_out = image::RgbaImage::from_raw(size, size, buf).unwrap();
        img_out
            .save_with_format(&out_path, ImageFormat::Png)
            .expect("Failed to write PNG");
    }

    println!("Created iconset at {}", output);
}

fn make_squircle_icon(source: &image::RgbaImage, size: u32) -> Vec<u8> {
    let scaled = imageops::resize(source, size, size, imageops::Lanczos3);
    let (sw, sh) = scaled.dimensions();
    let ox = (size - sw) / 2;
    let oy = (size - sh) / 2;
    let mut buf = vec![0u8; (size * size * 4) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let src = ((y * sw + x) * 4) as usize;
            let dst = (((oy + y) * size + (ox + x)) * 4) as usize;
            buf[dst..dst + 4].copy_from_slice(&scaled.as_raw()[src..src + 4]);
        }
    }
    let half = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let xf = (x as f32 + 0.5 - half) / half;
            let yf = (y as f32 + 0.5 - half) / half;
            let v = xf.abs().powi(5) + yf.abs().powi(5);
            if v > 1.0 {
                let i = (y * size + x) as usize * 4;
                buf[i + 3] = 0;
            }
        }
    }
    buf
}
