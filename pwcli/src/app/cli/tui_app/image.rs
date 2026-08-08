use std::env;
use std::io::{self, Write};

use anyhow::{Context, Result};
use base64::Engine;

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    Kitty,
    Iterm2,
    Text,
}

impl ImageProtocol {
    pub fn detect() -> Self {
        if env::var_os("TMUX").is_some() {
            return Self::Text;
        }
        let program = env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
        let term = env::var("TERM").unwrap_or_default().to_lowercase();
        if program.contains("ghostty")
            || program.contains("wezterm")
            || program.contains("kitty")
            || term.contains("kitty")
        {
            Self::Kitty
        } else if program.contains("iterm") {
            Self::Iterm2
        } else {
            Self::Text
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Kitty => "Kitty graphics",
            Self::Iterm2 => "iTerm2 images",
            Self::Text => "text fallback",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageArtifact {
    pub alt: String,
    pub source: String,
    pub protocol: ImageProtocol,
    data: Option<Vec<u8>>,
}

impl ImageArtifact {
    pub fn from_source(source: String, alt: String) -> Self {
        let data = decode_data_url(&source).ok();
        Self {
            alt,
            source,
            protocol: ImageProtocol::detect(),
            data,
        }
    }

    pub fn can_render(&self) -> bool {
        self.data.is_some() && self.protocol != ImageProtocol::Text
    }

    pub fn render(&self, width_cells: u16) -> Result<()> {
        let Some(data) = self.data.as_ref() else {
            return Ok(());
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
        let width = width_cells.clamp(10, 80);
        let sequence = match self.protocol {
            ImageProtocol::Kitty => format!(
                "\x1b7\x1b[3;3H\x1b_Ga=T,f=100,t=d,c={width},r=20,q=2;{encoded}\x1b\\\x1b8"
            ),
            ImageProtocol::Iterm2 => format!(
                "\x1b7\x1b[3;3H\x1b]1337;File=inline=1;width={width};preserveAspectRatio=1:{encoded}\x07\x1b8"
            ),
            ImageProtocol::Text => return Ok(()),
        };
        let mut stdout = io::stdout().lock();
        stdout.write_all(sequence.as_bytes())?;
        stdout.flush()?;
        Ok(())
    }
}

pub fn clear_terminal_images() {
    if ImageProtocol::detect() == ImageProtocol::Kitty {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\");
        let _ = stdout.flush();
    }
}

fn decode_data_url(source: &str) -> Result<Vec<u8>> {
    let (header, payload) = source.split_once(',').context("not a data URL")?;
    if !header.starts_with("data:image/") || !header.ends_with(";base64") {
        anyhow::bail!("unsupported image data URL");
    }
    if payload.len() > MAX_IMAGE_BYTES.saturating_mul(4) / 3 + 8 {
        anyhow::bail!("image exceeds size limit");
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .context("invalid image base64")?;
    if data.len() > MAX_IMAGE_BYTES {
        anyhow::bail!("image exceeds size limit");
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bounded_image_data_urls() {
        let source = "data:image/png;base64,iVBORw0KGgo=";
        let artifact = ImageArtifact::from_source(source.into(), "preview".into());
        assert!(artifact.data.is_some());
    }

    #[test]
    fn rejects_non_image_data_urls() {
        assert!(decode_data_url("data:text/plain;base64,aGk=").is_err());
    }
}
