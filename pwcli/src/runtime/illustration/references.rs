use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::{IllustrationMode, ReferenceEntry, ReferencePackManifest, RetrievedReference};

const STARTER_ZIP: &[u8] = include_bytes!("../../../resources/illustration/starter-pack.zip");
const MAX_ARCHIVE_BYTES: usize = 15 * 1024 * 1024;
const MAX_INSTALLED_BYTES: u64 = 24 * 1024 * 1024;

pub struct ReferenceStore {
    root: PathBuf,
}

impl ReferenceStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("illustration/reference-packs"),
        }
    }

    pub fn starter_archive_size() -> usize {
        STARTER_ZIP.len()
    }

    pub fn ensure_starter(&self) -> Result<ReferencePackManifest> {
        anyhow::ensure!(
            STARTER_ZIP.len() <= MAX_ARCHIVE_BYTES,
            "starter pack exceeds 15 MiB"
        );
        let target = self.root.join("starter-v1");
        if let Ok(manifest) = read_manifest(&target) {
            if validate_manifest(&target, &manifest).is_ok() {
                return Ok(manifest);
            }
            let _ = std::fs::remove_dir_all(&target);
        }
        std::fs::create_dir_all(&self.root)?;
        let staging = self.root.join(format!(".starter-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&staging)?;
        let mut archive = zip::ZipArchive::new(Cursor::new(STARTER_ZIP))?;
        let mut installed = 0u64;
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            let enclosed = file
                .enclosed_name()
                .context("starter pack contains unsafe path")?
                .to_owned();
            let destination = staging.join(enclosed);
            if file.is_dir() {
                std::fs::create_dir_all(&destination)?;
                continue;
            }
            installed = installed.saturating_add(file.size());
            anyhow::ensure!(
                installed <= MAX_INSTALLED_BYTES,
                "starter pack exceeds 24 MiB installed"
            );
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut bytes = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut bytes)?;
            std::fs::write(destination, bytes)?;
        }
        let manifest = read_manifest(&staging)?;
        validate_manifest(&staging, &manifest)?;
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::rename(&staging, &target)?;
        Ok(manifest)
    }

    pub fn retrieve(
        &self,
        mode: IllustrationMode,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievedReference>> {
        let query_tokens = tokens(query);
        let mut result = Vec::new();
        let personal_root = self.root.join("personal");
        if let Ok(personal) = read_manifest(&personal_root) {
            result.extend(retrieve_from_manifest(
                personal,
                &personal_root,
                mode,
                &query_tokens,
                limit,
            )?);
        }
        if result.len() < limit {
            let starter = self.ensure_starter()?;
            let root = self.root.join("starter-v1");
            let used = result
                .iter()
                .map(|r| r.entry.perceptual_hash.clone())
                .collect::<HashSet<_>>();
            result.extend(
                retrieve_from_manifest(starter, &root, mode, &query_tokens, limit - result.len())?
                    .into_iter()
                    .filter(|r| !used.contains(&r.entry.perceptual_hash)),
            );
        }
        result.truncate(limit);
        Ok(result)
    }

    pub fn status(&self) -> Result<serde_json::Value> {
        let manifest = self.ensure_starter()?;
        Ok(serde_json::json!({
            "id": manifest.id, "version": manifest.version,
            "entries": manifest.entries.len(), "archiveBytes": STARTER_ZIP.len(),
            "installedPath": self.root.join("starter-v1")
        }))
    }

    pub fn reset_starter(&self) -> Result<()> {
        let target = self.root.join("starter-v1");
        if target.exists() {
            std::fs::remove_dir_all(target)?;
        }
        self.ensure_starter().map(|_| ())
    }

    pub fn list_personal(&self) -> Result<Vec<ReferenceEntry>> {
        Ok(read_manifest(&self.root.join("personal"))
            .map(|m| m.entries)
            .unwrap_or_default())
    }

    pub fn import_personal(
        &self,
        path: &Path,
        kind: IllustrationMode,
        layout: &str,
        visual_intent: &str,
        content_summary: &str,
    ) -> Result<ReferenceEntry> {
        anyhow::ensure!(
            matches!(kind, IllustrationMode::Diagram | IllustrationMode::Plot),
            "personal reference kind must be diagram or plot"
        );
        let source = std::fs::read(path)
            .with_context(|| format!("read personal reference {}", path.display()))?;
        let image = image::load_from_memory(&source)
            .context("personal reference is not a supported raster image")?;
        let image = if image.width() > 768 || image.height() > 768 {
            image.resize(768, 768, image::imageops::FilterType::Lanczos3)
        } else {
            image
        };
        let rgb = image.into_rgb8();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 76).encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        anyhow::ensure!(
            bytes.len() <= 250 * 1024,
            "personal reference remains above 250 KiB after resizing; crop or simplify it first"
        );
        let id = format!("personal-{}", uuid::Uuid::now_v7());
        let sha = hex::encode(Sha256::digest(&bytes));
        let personal_root = self.root.join("personal");
        std::fs::create_dir_all(personal_root.join("images"))?;
        let image_path = format!("images/{id}.jpg");
        std::fs::write(personal_root.join(&image_path), &bytes)?;
        let entry = ReferenceEntry {
            id: id.clone(),
            kind,
            layout: layout.into(),
            content_summary: content_summary.into(),
            visual_intent: visual_intent.into(),
            keywords: tokens(&format!("{layout} {visual_intent} {content_summary}"))
                .into_iter()
                .collect(),
            image_path,
            media_type: "image/jpeg".into(),
            width: rgb.width(),
            height: rgb.height(),
            sha256: sha.clone(),
            perceptual_hash: sha[..24].into(),
            source: path.display().to_string(),
            author: "pwcli user".into(),
            license: "user-owned/local-only".into(),
            redistributable: false,
            notice: "User explicitly imported this local-only reference.".into(),
        };
        let mut manifest = read_manifest(&personal_root).unwrap_or(ReferencePackManifest {
            schema_version: 1,
            id: "personal".into(),
            version: "1".into(),
            license_notice: "User-managed local-only references".into(),
            entries: vec![],
        });
        manifest.entries.push(entry.clone());
        write_manifest(&personal_root, &manifest)?;
        Ok(entry)
    }

    pub fn remove_personal(&self, id: &str) -> Result<()> {
        let root = self.root.join("personal");
        let mut manifest = read_manifest(&root).context("personal reference library is empty")?;
        let index = manifest
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .context("personal reference not found")?;
        let entry = manifest.entries.remove(index);
        let path = root.join(entry.image_path);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        write_manifest(&root, &manifest)
    }

    /// Import only the pools explicitly enumerated by PaperBananaBench's
    /// `ref.json` files. Benchmark test/GT files outside those manifests are
    /// never traversed or copied.
    pub fn import_paperbanana_reference_pool(&self, root: &Path) -> Result<usize> {
        let mut imported = 0usize;
        for (directory, kind) in [
            ("diagram", IllustrationMode::Diagram),
            ("plot", IllustrationMode::Plot),
        ] {
            let task_root = root.join(directory);
            let ref_manifest = task_root.join("ref.json");
            if !ref_manifest.is_file() {
                continue;
            }
            let entries: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(ref_manifest)?)?;
            for value in entries {
                let relative = value
                    .get("path_to_gt_image")
                    .and_then(serde_json::Value::as_str)
                    .context("PaperBanana ref entry lacks path_to_gt_image")?;
                let relative = Path::new(relative);
                anyhow::ensure!(
                    !relative.is_absolute()
                        && !relative
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir)),
                    "PaperBanana ref entry contains an unsafe image path"
                );
                let intent = value
                    .get("visual_intent")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Imported PaperBanana reference");
                let summary = value
                    .get("content")
                    .map(|content| {
                        content
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| content.to_string())
                    })
                    .unwrap_or_default();
                self.import_personal(
                    &task_root.join(relative),
                    kind,
                    "paperbanana-reference",
                    intent,
                    &summary,
                )?;
                imported += 1;
            }
        }
        anyhow::ensure!(
            imported > 0,
            "no diagram/ref.json or plot/ref.json reference entries found"
        );
        Ok(imported)
    }
}

fn retrieve_from_manifest(
    manifest: ReferencePackManifest,
    root: &Path,
    mode: IllustrationMode,
    query_tokens: &HashSet<String>,
    limit: usize,
) -> Result<Vec<RetrievedReference>> {
    let mut scored = manifest
        .entries
        .into_iter()
        .filter(|entry| entry.kind == mode)
        .map(|entry| {
            let haystack = format!(
                "{} {} {} {}",
                entry.layout,
                entry.content_summary,
                entry.visual_intent,
                entry.keywords.join(" ")
            );
            (tokens(&haystack).intersection(query_tokens).count(), entry)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, entry)| {
            Ok(RetrievedReference {
                bytes: std::fs::read(root.join(&entry.image_path))?,
                entry,
            })
        })
        .collect()
}

fn write_manifest(root: &Path, manifest: &ReferencePackManifest) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let temporary = root.join(".manifest.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(manifest)?)?;
    std::fs::rename(temporary, root.join("manifest.json"))?;
    Ok(())
}

fn read_manifest(root: &Path) -> Result<ReferencePackManifest> {
    Ok(serde_json::from_slice(&std::fs::read(
        root.join("manifest.json"),
    )?)?)
}

fn validate_manifest(root: &Path, manifest: &ReferencePackManifest) -> Result<()> {
    anyhow::ensure!(
        manifest.schema_version == 1,
        "unsupported reference pack schema"
    );
    anyhow::ensure!(
        manifest.entries.len() == 96,
        "starter pack must contain 96 entries"
    );
    let diagrams = manifest
        .entries
        .iter()
        .filter(|e| e.kind == IllustrationMode::Diagram)
        .count();
    let plots = manifest
        .entries
        .iter()
        .filter(|e| e.kind == IllustrationMode::Plot)
        .count();
    anyhow::ensure!(diagrams == 64 && plots == 32, "starter pack quota mismatch");
    let mut ids = HashSet::new();
    let mut hashes = HashSet::new();
    for entry in &manifest.entries {
        anyhow::ensure!(
            entry.redistributable
                && !entry.license.trim().is_empty()
                && !entry.author.trim().is_empty()
                && !entry.source.trim().is_empty(),
            "reference {} lacks redistribution metadata",
            entry.id
        );
        anyhow::ensure!(
            entry.width <= 768 && entry.height <= 768,
            "reference {} exceeds 768px",
            entry.id
        );
        anyhow::ensure!(
            ids.insert(&entry.id) && hashes.insert(&entry.perceptual_hash),
            "duplicate reference {}",
            entry.id
        );
        let bytes = std::fs::read(root.join(&entry.image_path))?;
        anyhow::ensure!(
            bytes.len() <= 250 * 1024,
            "reference {} exceeds 250 KiB",
            entry.id
        );
        let actual = hex::encode(Sha256::digest(&bytes));
        anyhow::ensure!(
            actual == entry.sha256,
            "reference {} checksum mismatch",
            entry.id
        );
    }
    Ok(())
}

fn tokens(value: &str) -> HashSet<String> {
    value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() > 1)
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_starter_pack_meets_release_contract() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReferenceStore::new(temp.path());
        let manifest = store.ensure_starter().unwrap();
        assert_eq!(manifest.entries.len(), 96);
        assert_eq!(
            manifest
                .entries
                .iter()
                .filter(|e| e.kind == IllustrationMode::Diagram)
                .count(),
            64
        );
        assert_eq!(
            manifest
                .entries
                .iter()
                .filter(|e| e.kind == IllustrationMode::Plot)
                .count(),
            32
        );
        assert!(ReferenceStore::starter_archive_size() <= 15 * 1024 * 1024);
        assert!(manifest
            .entries
            .iter()
            .all(|entry| entry.redistributable && entry.media_type == "image/webp"));
    }

    #[test]
    fn starter_retrieval_is_deterministic_and_type_safe() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReferenceStore::new(temp.path());
        let first = store
            .retrieve(IllustrationMode::Diagram, "retrieval memory pipeline", 10)
            .unwrap();
        let second = store
            .retrieve(IllustrationMode::Diagram, "retrieval memory pipeline", 10)
            .unwrap();
        assert_eq!(
            first.iter().map(|r| &r.entry.id).collect::<Vec<_>>(),
            second.iter().map(|r| &r.entry.id).collect::<Vec<_>>()
        );
        assert!(first
            .iter()
            .all(|reference| reference.entry.kind == IllustrationMode::Diagram));
    }
}
