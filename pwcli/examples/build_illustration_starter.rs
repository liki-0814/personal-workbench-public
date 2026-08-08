//! Deterministically builds pwcli's project-owned illustration starter pack.
//! Run from `pwcli/` with `cargo run --example build_illustration_starter`.

use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;

use anyhow::Result;
use image::codecs::webp::WebPEncoder;
use image::{ImageBuffer, ImageEncoder, Rgba};
use serde_json::json;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const W: u32 = 768;
const H: u32 = 480;

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/illustration");
    fs::create_dir_all(&root)?;
    let diagram_layouts = [
        "linear-pipeline",
        "branching-multistage",
        "cyclic-iterative",
        "encoder-decoder",
        "retrieval-memory",
        "multi-agent",
        "multimodal-fusion",
        "training-inference-split",
    ];
    let plot_layouts = [
        "bar",
        "line",
        "scatter",
        "distribution",
        "heatmap",
        "donut",
        "radar",
        "multi-panel",
    ];
    let palettes = [
        [
            [33, 90, 166],
            [85, 160, 204],
            [111, 186, 130],
            [242, 180, 77],
        ],
        [
            [88, 71, 135],
            [166, 92, 151],
            [220, 138, 96],
            [239, 196, 101],
        ],
        [
            [34, 110, 105],
            [72, 154, 143],
            [150, 190, 145],
            [231, 184, 93],
        ],
        [
            [57, 74, 105],
            [77, 126, 168],
            [108, 169, 157],
            [231, 161, 93],
        ],
    ];
    let mut files = Vec::new();
    let mut entries = Vec::new();

    for (layout_index, layout) in diagram_layouts.iter().enumerate() {
        for variant in 0..8usize {
            let id = format!("diagram-{layout}-{:02}", variant + 1);
            let image = diagram(layout_index, variant, &palettes[variant % palettes.len()]);
            let bytes = encode_webp(image)?;
            push_entry(
                &mut files,
                &mut entries,
                &id,
                "diagram",
                layout,
                variant,
                bytes,
            );
        }
    }
    for (layout_index, layout) in plot_layouts.iter().enumerate() {
        for variant in 0..4usize {
            let id = format!("plot-{layout}-{:02}", variant + 1);
            let image = plot(layout_index, variant, &palettes[variant % palettes.len()]);
            let bytes = encode_webp(image)?;
            push_entry(
                &mut files,
                &mut entries,
                &id,
                "plot",
                layout,
                variant,
                bytes,
            );
        }
    }

    let manifest = serde_json::to_vec_pretty(&json!({
        "schemaVersion": 1,
        "id": "pwcli-starter",
        "version": "1.0.0",
        "licenseNotice": "Project-owned synthetic reference compositions, AGPL-3.0-only; no PaperBananaBench images or benchmark ground truth.",
        "entries": entries
    }))?;
    let license = b"PWCLI Illustration Starter Pack\n\nCopyright 2026 pwcli contributors.\nProject-owned synthetic visual compositions licensed AGPL-3.0-only.\nNo PaperBananaBench images, paper figures, trademarks, or benchmark ground truth are included.\n";
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    archive.start_file("manifest.json", options)?;
    archive.write_all(&manifest)?;
    archive.start_file("LICENSE.txt", options)?;
    archive.write_all(license)?;
    for (path, bytes) in files {
        archive.start_file(path, options)?;
        archive.write_all(&bytes)?;
    }
    let bytes = archive.finish()?.into_inner();
    anyhow::ensure!(bytes.len() <= 15 * 1024 * 1024, "archive exceeds 15 MiB");
    fs::write(root.join("starter-pack.zip"), &bytes)?;
    println!("wrote {} bytes and 96 references", bytes.len());
    Ok(())
}

fn push_entry(
    files: &mut Vec<(String, Vec<u8>)>,
    entries: &mut Vec<serde_json::Value>,
    id: &str,
    kind: &str,
    layout: &str,
    variant: usize,
    bytes: Vec<u8>,
) {
    let image_path = format!("images/{id}.webp");
    let sha = hex::encode(Sha256::digest(&bytes));
    entries.push(json!({
        "id": id, "kind": kind, "layout": layout,
        "contentSummary": format!("Synthetic {layout} scientific illustration composition variant {}", variant + 1),
        "visualIntent": format!("Clear publication-ready {layout} layout with restrained color and strong hierarchy"),
        "keywords": [layout, "academic", "scientific", "publication", "clean", "reference"],
        "imagePath": image_path, "mediaType": "image/webp", "width": W, "height": H,
        "sha256": sha, "perceptualHash": &sha[..24],
        "source": "pwcli deterministic starter-pack generator", "author": "pwcli contributors",
        "license": "AGPL-3.0-only", "redistributable": true,
        "notice": "Project-owned synthetic composition; not derived from PaperBananaBench."
    }));
    files.push((image_path, bytes));
}

fn canvas() -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    ImageBuffer::from_pixel(W, H, Rgba([248, 250, 252, 255]))
}
fn color(rgb: [u8; 3]) -> Rgba<u8> {
    Rgba([rgb[0], rgb[1], rgb[2], 255])
}
fn rect(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    for yy in y..(y + h).min(H) {
        for xx in x..(x + w).min(W) {
            img.put_pixel(xx, yy, c);
        }
    }
}
fn line(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    c: Rgba<u8>,
    thick: i32,
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        for oy in -thick..=thick {
            for ox in -thick..=thick {
                let x = x0 + ox;
                let y = y0 + oy;
                if x >= 0 && y >= 0 && (x as u32) < W && (y as u32) < H {
                    img.put_pixel(x as u32, y as u32, c);
                }
            }
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx
        }
        if e2 <= dx {
            err += dx;
            y0 += sy
        }
    }
}
fn node(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    rect(img, x, y, w, h, Rgba([255, 255, 255, 255]));
    rect(img, x, y, w, 5, c);
    rect(img, x, y, 5, h, c);
    rect(img, x + w - 5, y, 5, h, c);
    rect(img, x, y + h - 5, w, 5, c);
}
fn variant_signature(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>, variant: usize, c: Rgba<u8>) {
    for bit in 0..8 {
        if (variant + 1) & (1 << bit) != 0 {
            rect(img, 710 + bit as u32 * 6, 18, 4, 9, c);
        }
    }
}

fn diagram(
    layout: usize,
    variant: usize,
    palette: &[[u8; 3]; 4],
) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let mut img = canvas();
    let ink = Rgba([55, 65, 81, 255]);
    match layout {
        0 => {
            for i in 0..4 {
                let x = 45 + i * 180;
                node(&mut img, x, 180, 130, 95, color(palette[i as usize]));
                if i < 3 {
                    line(
                        &mut img,
                        (x + 130) as i32,
                        227,
                        (x + 180) as i32,
                        227,
                        ink,
                        2,
                    );
                }
            }
        }
        1 => {
            node(&mut img, 55, 190, 130, 90, color(palette[0]));
            for i in 0..3 {
                let y = 65 + i * 145;
                node(&mut img, 315, y, 145, 80, color(palette[i as usize + 1]));
                line(&mut img, 185, 235, 315, (y + 40) as i32, ink, 2);
            }
            node(&mut img, 585, 190, 130, 90, color(palette[0]));
        }
        2 => {
            let pts = [(300, 55), (510, 185), (430, 350), (170, 350), (90, 185)];
            for (i, (x, y)) in pts.iter().enumerate() {
                node(&mut img, *x, *y, 110, 65, color(palette[i % 4]));
                let (nx, ny) = pts[(i + 1) % pts.len()];
                line(
                    &mut img,
                    (x + 55) as i32,
                    (y + 32) as i32,
                    (nx + 55) as i32,
                    (ny + 32) as i32,
                    ink,
                    2,
                );
            }
        }
        3 => {
            for i in 0..3 {
                node(
                    &mut img,
                    50 + i * 130,
                    175 + i * 25,
                    105,
                    80,
                    color(palette[i as usize]),
                );
            }
            for i in 0..3 {
                node(
                    &mut img,
                    610 - i * 130,
                    225 - i * 25,
                    105,
                    80,
                    color(palette[(i + 1) as usize]),
                );
            }
            node(&mut img, 335, 170, 100, 140, color(palette[3]));
        }
        4 => {
            node(&mut img, 55, 180, 135, 95, color(palette[0]));
            node(&mut img, 315, 65, 150, 80, color(palette[1]));
            node(&mut img, 315, 330, 150, 80, color(palette[2]));
            node(&mut img, 580, 180, 135, 95, color(palette[3]));
            line(&mut img, 190, 225, 315, 105, ink, 2);
            line(&mut img, 190, 225, 315, 370, ink, 2);
            line(&mut img, 465, 105, 580, 225, ink, 2);
            line(&mut img, 465, 370, 580, 225, ink, 2);
        }
        5 => {
            node(&mut img, 300, 185, 165, 105, color(palette[0]));
            for i in 0..4 {
                let x = 55 + (i % 2) * 550;
                let y = 65 + (i / 2) * 300;
                node(&mut img, x, y, 115, 70, color(palette[i as usize]));
                line(&mut img, (x + 57) as i32, (y + 35) as i32, 382, 237, ink, 2);
            }
        }
        6 => {
            for i in 0..3 {
                node(
                    &mut img,
                    45,
                    55 + i * 145,
                    135,
                    80,
                    color(palette[i as usize]),
                );
                line(&mut img, 180, (95 + i * 145) as i32, 335, 235, ink, 2);
            }
            node(&mut img, 335, 165, 120, 140, color(palette[3]));
            node(&mut img, 590, 185, 130, 100, color(palette[0]));
            line(&mut img, 455, 235, 590, 235, ink, 2);
        }
        _ => {
            rect(&mut img, 35, 35, 335, 410, Rgba([239, 246, 255, 255]));
            rect(&mut img, 398, 35, 335, 410, Rgba([255, 247, 237, 255]));
            for i in 0..3 {
                node(
                    &mut img,
                    75,
                    90 + i * 115,
                    250,
                    70,
                    color(palette[i as usize]),
                );
                node(
                    &mut img,
                    440,
                    90 + i * 115,
                    250,
                    70,
                    color(palette[(i + 1) as usize]),
                );
            }
        }
    }
    if variant % 2 == 1 {
        rect(&mut img, 30, 20, 140, 7, color(palette[variant % 4]));
    }
    variant_signature(&mut img, variant, color(palette[variant % 4]));
    img
}

fn plot(kind: usize, variant: usize, palette: &[[u8; 3]; 4]) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let mut img = canvas();
    let ink = Rgba([55, 65, 81, 255]);
    line(&mut img, 85, 45, 85, 405, ink, 2);
    line(&mut img, 85, 405, 720, 405, ink, 2);
    match kind {
        0 => {
            for i in 0..6u32 {
                let h = 80 + ((i as usize * 43 + variant * 31) % 250) as u32;
                rect(
                    &mut img,
                    120 + i * 90,
                    405 - h,
                    55,
                    h,
                    color(palette[i as usize % 4]),
                );
            }
        }
        1 => {
            for s in 0..3usize {
                let c = color(palette[s]);
                let mut last = (110i32, 320 - s as i32 * 45);
                for i in 1..7i32 {
                    let p = (
                        110 + i * 85,
                        320 - s as i32 * 45
                            - ((i * 37 + s as i32 * 29 + variant as i32 * 17) % 130),
                    );
                    line(&mut img, last.0, last.1, p.0, p.1, c, 2);
                    rect(&mut img, (p.0 - 4) as u32, (p.1 - 4) as u32, 9, 9, c);
                    last = p;
                }
            }
        }
        2 => {
            for i in 0..55 {
                let x = 115 + ((i * 79 + variant * 17) % 570) as u32;
                let y = 80 + ((i * 47 + i * i * 3 + variant * 23) % 290) as u32;
                rect(&mut img, x, y, 7, 7, color(palette[i as usize % 4]));
            }
        }
        3 => {
            for i in 0..24 {
                let x = 105 + i * 24;
                let d = (i as i32 - 12).abs() as u32;
                let h = 20 + (220u32.saturating_sub(d * d));
                rect(&mut img, x, 405 - h, 20, h, color(palette[variant % 4]));
            }
        }
        4 => {
            for r in 0..6u32 {
                for c in 0..8u32 {
                    let p = palette[(r as usize + c as usize + variant) % 4];
                    let fade = 60 + ((r * 8 + c) * 3) as u8;
                    rect(
                        &mut img,
                        125 + c * 68,
                        65 + r * 52,
                        58,
                        42,
                        Rgba([
                            p[0].saturating_add(fade / 5),
                            p[1].saturating_add(fade / 5),
                            p[2].saturating_add(fade / 5),
                            255,
                        ]),
                    );
                }
            }
        }
        5 => {
            let cx = 390i32;
            let cy = 230i32;
            for y in 55..405u32 {
                for x in 210..570u32 {
                    let dx = x as i32 - cx;
                    let dy = y as i32 - cy;
                    let d = dx * dx + dy * dy;
                    if d < 170 * 170 && d > 80 * 80 {
                        let angle = (dy as f32).atan2(dx as f32);
                        let quadrant = ((angle + std::f32::consts::PI) * 2.0 / std::f32::consts::PI)
                            as usize
                            % 4;
                        img.put_pixel(x, y, color(palette[(quadrant + variant) % 4]));
                    }
                }
            }
        }
        6 => {
            let cx = 390;
            let cy = 225;
            for arm in 0..6 {
                let a = arm as f32 * std::f32::consts::PI / 3.0;
                line(
                    &mut img,
                    cx,
                    cy,
                    cx + (a.cos() * 170.0) as i32,
                    cy + (a.sin() * 170.0) as i32,
                    ink,
                    1,
                );
            }
            for ring in [55.0, 105.0, 155.0] {
                let mut last: Option<(i32, i32)> = None;
                for i in 0..=60 {
                    let a = i as f32 * std::f32::consts::PI / 30.0;
                    let p = (cx + (a.cos() * ring) as i32, cy + (a.sin() * ring) as i32);
                    if let Some(q) = last {
                        line(&mut img, q.0, q.1, p.0, p.1, Rgba([180, 190, 205, 255]), 1);
                    }
                    last = Some(p);
                }
            }
        }
        _ => {
            for panel in 0..4u32 {
                let x = 105 + (panel % 2) * 310;
                let y = 65 + (panel / 2) * 170;
                rect(&mut img, x, y, 280, 140, Rgba([255, 255, 255, 255]));
                for i in 0..5u32 {
                    let h =
                        25 + ((i as usize * 23 + panel as usize * 31 + variant * 17) % 100) as u32;
                    rect(
                        &mut img,
                        x + 25 + i * 48,
                        y + 125 - h,
                        28,
                        h,
                        color(palette[(i + panel) as usize % 4]),
                    );
                }
            }
        }
    }
    variant_signature(&mut img, variant, color(palette[variant % 4]));
    img
}

fn encode_webp(image: ImageBuffer<Rgba<u8>, Vec<u8>>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    WebPEncoder::new_lossless(&mut bytes).write_image(
        image.as_raw(),
        W,
        H,
        image::ExtendedColorType::Rgba8,
    )?;
    anyhow::ensure!(
        bytes.len() <= 250 * 1024,
        "generated reference exceeds 250 KiB"
    );
    Ok(bytes)
}
