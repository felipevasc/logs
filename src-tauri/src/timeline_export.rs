//! Save a rendered timeline through a native picker, never an arbitrary JS path.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_BASE64: usize = MAX_BYTES.div_ceil(3) * 4;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Png,
    Pdf,
}
impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
    fn accepts(self, bytes: &[u8]) -> bool {
        match self {
            Self::Png => {
                bytes.len() >= 45
                    && bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
                    && bytes.ends_with(b"\0\0\0\0IEND\xae\x42\x60\x82")
            }
            Self::Pdf => {
                (bytes.starts_with(b"%PDF-1.") || bytes.starts_with(b"%PDF-2."))
                    && bytes.trim_ascii_end().ends_with(b"%%EOF")
            }
        }
    }
}

#[derive(Serialize)]
pub struct ExportResult {
    saved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<usize>,
}

fn suggested_name(filename: &str, format: Format) -> String {
    let basename = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = Path::new(basename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let stem: String = stem
        .chars()
        .filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c))
        .take(100)
        .collect();
    let stem = stem.trim_matches([' ', '.']);
    let reserved = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    format!(
        "{}.{}",
        if stem.is_empty() || reserved {
            "timeline"
        } else {
            stem
        },
        format.extension()
    )
}

fn decode(format: Format, encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() > MAX_BASE64 {
        return Err("A exportação excede 32 MiB. Reduza o intervalo ou a escala da imagem.".into());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "A exportação não contém base64 válido.")?;
    if bytes.len() > MAX_BYTES {
        return Err("A exportação excede 32 MiB. Reduza o intervalo ou a escala da imagem.".into());
    }
    if !format.accepts(&bytes) {
        return Err(format!(
            "O conteúdo não é um {} completo reconhecido.",
            format.extension().to_uppercase()
        ));
    }
    Ok(bytes)
}

fn save_atomic(path: &Path, format: Format, bytes: &[u8]) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .extension()
            .and_then(|e| e.to_str())
            .is_none_or(|e| !e.eq_ignore_ascii_case(format.extension()))
    {
        return Err(format!(
            "Escolha um nome de arquivo com a extensão .{}.",
            format.extension()
        ));
    }
    let parent = path.parent().ok_or("Pasta de destino inválida.")?;
    crate::operations::check()?;
    let mut pending = tempfile::Builder::new()
        .prefix(".loginsight-timeline-")
        .suffix(".part")
        .tempfile_in(parent)
        .map_err(|e| format!("Não foi possível preparar o arquivo de exportação: {e}"))?;
    for chunk in bytes.chunks(512 * 1024) {
        crate::operations::check()?;
        pending
            .write_all(chunk)
            .map_err(|e| format!("Não foi possível gravar a exportação: {e}"))?;
    }
    pending
        .as_file()
        .sync_all()
        .map_err(|e| format!("Não foi possível concluir a exportação: {e}"))?;
    crate::operations::check()?;
    // The temp file shares the destination filesystem. persist atomically replaces
    // an existing file only after the native picker has confirmed that choice.
    pending
        .persist(path)
        .map_err(|e| format!("Não foi possível publicar a exportação: {}", e.error))?;
    crate::operations::commit();
    Ok(())
}

fn export_impl(
    format: Format,
    filename: &str,
    encoded: &str,
    choose: impl FnOnce(&str, &str) -> Result<Option<PathBuf>, String>,
) -> Result<ExportResult, String> {
    let bytes = decode(format, encoded)?;
    crate::operations::check()?;
    let Some(path) = choose(&suggested_name(filename, format), format.extension())? else {
        crate::operations::commit();
        return Ok(ExportResult {
            saved: false,
            path: None,
            bytes: None,
        });
    };
    save_atomic(&path, format, &bytes)?;
    Ok(ExportResult {
        saved: true,
        path: Some(path.to_string_lossy().into_owned()),
        bytes: Some(bytes.len()),
    })
}

#[tauri::command]
pub async fn export_timeline(
    format: Format,
    filename: String,
    base64: String,
    app: tauri::AppHandle,
) -> Result<ExportResult, String> {
    crate::offload(move || {
        export_impl(format, &filename, &base64, |name, extension| {
            let mut picker = app
                .dialog()
                .file()
                .set_title("Exportar linha do tempo")
                .set_file_name(name)
                .add_filter(extension.to_uppercase(), &[extension]);
            if let Some(window) = app.get_webview_window("main") {
                picker = picker.set_parent(&window);
            }
            picker
                .blocking_save_file()
                .map(|path| {
                    path.into_path().map_err(|_| {
                        "Escolha um arquivo local para salvar a exportação.".to_owned()
                    })
                })
                .transpose()
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Zl1sAAAAASUVORK5CYII=";
    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
    #[test]
    fn timeline_export_validates_and_cancels_without_writing() {
        let cancelled = export_impl(Format::Png, "timeline.png", PNG, |name, ext| {
            assert_eq!(name, "timeline.png");
            assert_eq!(ext, "png");
            Ok(None)
        })
        .unwrap();
        assert!(!cancelled.saved);
        assert!(cancelled.path.is_none());
        assert_eq!(
            serde_json::to_value(cancelled).unwrap(),
            serde_json::json!({"saved":false})
        );
        assert!(decode(Format::Pdf, PNG).is_err());
        assert!(decode(Format::Png, &STANDARD.encode(PDF)).is_err());
        assert!(decode(Format::Png, "data:image/png;base64,AAAA").is_err());
        assert!(decode(Format::Pdf, &STANDARD.encode(b"%PDF-1.4\ntruncated")).is_err());
        assert!(decode(Format::Pdf, &"A".repeat(MAX_BASE64 + 1))
            .unwrap_err()
            .contains("32 MiB"));
        assert_eq!(
            suggested_name("../../something.exe", Format::Png),
            "something.png"
        );
        assert_eq!(suggested_name("CON", Format::Pdf), "timeline.pdf");
    }
    #[test]
    fn timeline_export_replaces_atomically_and_preserves_destination_on_failure() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("timeline.pdf");
        std::fs::write(&path, "old file").unwrap();
        let invalid = export_impl(Format::Pdf, "timeline.pdf", PNG, |_, _| {
            panic!("invalid data must not show picker")
        });
        assert!(invalid.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old file");
        let result = export_impl(
            Format::Pdf,
            "timeline.pdf",
            &STANDARD.encode(PDF),
            |_, _| Ok(Some(path.clone())),
        )
        .unwrap();
        assert!(result.saved);
        assert_eq!(result.bytes, Some(PDF.len()));
        assert_eq!(std::fs::read(&path).unwrap(), PDF);
        assert!(save_atomic(
            &folder.path().join("wrong.exe"),
            Format::Png,
            &decode(Format::Png, PNG).unwrap()
        )
        .is_err());
        let blocked = folder.path().join("directory.pdf");
        std::fs::create_dir(&blocked).unwrap();
        assert!(save_atomic(&blocked, Format::Pdf, PDF).is_err());
        assert!(blocked.is_dir());
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 2);
    }
}
