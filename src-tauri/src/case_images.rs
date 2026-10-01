//! Content-addressed local evidence images. Case JSON stores references only.
use base64::{engine::general_purpose::STANDARD, Engine};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

const IMAGE_BYTES: usize = 16 * 1024 * 1024;
const IMAGE_PIXELS: u64 = 16_000_000;
const DOCUMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGES: usize = 1000;
static IMAGE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Serialize)]
pub struct ImageInfo {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub bytes: usize,
    pub width: u32,
    pub height: u32,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    data_url: String,
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Identificador de imagem inválido.".into());
    }
    Ok(())
}
fn image_dir(root: &Path) -> Result<PathBuf, String> {
    let path = root.join("case-images");
    fs::create_dir_all(&path)
        .map_err(|e| format!("Não foi possível preparar as imagens do Caso: {e}"))?;
    if fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("A pasta de imagens não pode ser um atalho simbólico.".into());
    }
    path.canonicalize().map_err(|e| e.to_string())
}
fn contained_file(directory: &Path, id: &str, thumbnail: bool) -> Result<PathBuf, String> {
    valid_id(id)?;
    let path = directory.join(if thumbnail {
        format!("{id}.thumb.png")
    } else {
        id.to_owned()
    });
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || path.canonicalize().map_err(|e| e.to_string())?.parent() != Some(directory)
        {
            return Err("Referência de imagem fora do armazenamento do Caso.".into());
        }
    }
    Ok(path)
}
fn read_bytes(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let file = crate::case_archive_format::open_regular(path).map_err(|error| {
        format!(
        "Imagem do Caso não encontrada. Importe a investigação com os anexos originais. ({error})"
    )
    })?;
    if file.metadata().map_err(|e| e.to_string())?.len() > max as u64 {
        return Err("Imagem excede o limite permitido.".into());
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max {
        return Err("Imagem excede o limite permitido.".into());
    }
    Ok(bytes)
}
fn decode_base64(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() > IMAGE_BYTES.div_ceil(3) * 4 {
        return Err("Cada imagem deve ter no máximo 16 MiB.".into());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "A imagem não contém base64 válido.".to_string())?;
    if bytes.is_empty() || bytes.len() > IMAGE_BYTES {
        return Err("Cada imagem deve ter entre 1 byte e 16 MiB.".into());
    }
    Ok(bytes)
}
fn check_static(bytes: &[u8], format: ImageFormat) -> Result<(), String> {
    // Reject multi-frame files: a small compressed animation can contain many
    // full-size frames even though its first frame meets the pixel budget.
    let (mut cursor, little) = match format {
        ImageFormat::Png => (8, false),
        ImageFormat::WebP => (12, true),
        ImageFormat::Jpeg => {
            return if bytes.ends_with(&[0xff, 0xd9]) {
                Ok(())
            } else {
                Err("JPEG incompleto.".into())
            }
        }
        _ => return Err("Use uma imagem PNG, JPEG ou WebP.".into()),
    };
    if little
        && (bytes.len() < 12
            || (u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize).checked_add(8)
                != Some(bytes.len()))
    {
        return Err("WebP incompleto.".into());
    }
    while cursor + 8 <= bytes.len() {
        let (kind, size) = if little {
            (
                &bytes[cursor..cursor + 4],
                u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize,
            )
        } else {
            (
                &bytes[cursor + 4..cursor + 8],
                u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize,
            )
        };
        if kind == b"acTL" || kind == b"ANIM" || kind == b"ANMF" {
            return Err("Use uma imagem estática; animações não são aceitas como prints.".into());
        }
        let end = cursor
            .checked_add(size)
            .and_then(|n| n.checked_add(if little { 8 + size % 2 } else { 12 }))
            .ok_or("Imagem inválida.")?;
        if end > bytes.len() {
            return Err("Imagem incompleta.".into());
        }
        if !little && kind == b"IEND" {
            return if size == 0 && end == bytes.len() {
                Ok(())
            } else {
                Err("PNG contém dados após o final da imagem.".into())
            };
        }
        cursor = end;
    }
    if little && cursor == bytes.len() {
        Ok(())
    } else {
        Err("Imagem incompleta.".into())
    }
}
fn inspect(bytes: &[u8]) -> Result<(&'static str, DynamicImage), String> {
    crate::operations::check()?;
    if bytes.is_empty() || bytes.len() > IMAGE_BYTES {
        return Err("Cada imagem deve ter no máximo 16 MiB.".into());
    }
    let format =
        image::guess_format(bytes).map_err(|_| "Formato de imagem não reconhecido.".to_string())?;
    let mime = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        _ => return Err("Use uma imagem PNG, JPEG ou WebP.".into()),
    };
    check_static(bytes, format)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_000);
    limits.max_image_height = Some(16_000);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let reader = || {
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        reader.limits(limits.clone());
        reader
    };
    let (width, height) = reader()
        .into_dimensions()
        .map_err(|_| "Cabeçalho de imagem inválido ou dimensões excessivas.".to_string())?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > IMAGE_PIXELS {
        return Err("A imagem excede 16 megapixels. Reduza suas dimensões antes de anexar.".into());
    }
    let decoded = reader().decode().map_err(|_| {
        "Não foi possível decodificar a imagem completa dentro dos limites permitidos.".to_string()
    })?;
    crate::operations::check()?;
    Ok((mime, decoded))
}
fn name(name: &str) -> String {
    let value: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect();
    if value.trim().is_empty() {
        "Imagem".into()
    } else {
        value
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn persist(directory: &Path, id: &str, bytes: &[u8], thumbnail: bool) -> Result<(), String> {
    let path = contained_file(directory, id, thumbnail)?;
    if path.exists() {
        if read_bytes(&path, IMAGE_BYTES)? != bytes {
            return Err("Uma imagem existente está corrompida; o original foi preservado.".into());
        }
        return Ok(());
    }
    let mut temp = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    crate::operations::check()?;
    match temp.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_bytes(&path, IMAGE_BYTES)? == bytes {
                Ok(())
            } else {
                Err("Imagem existente diferente; nenhum original foi substituído.".into())
            }
        }
        Err(e) => Err(e.error.to_string()),
    }
}
fn add_at(root: &Path, encoded: &str, filename: &str) -> Result<ImageInfo, String> {
    let bytes = decode_base64(encoded)?;
    let (mime, decoded) = inspect(&bytes)?;
    let info = ImageInfo {
        id: hash(&bytes),
        name: name(filename),
        mime: mime.into(),
        bytes: bytes.len(),
        width: decoded.width(),
        height: decoded.height(),
    };
    drop(decoded);
    persist(&image_dir(root)?, &info.id, &bytes, false)?;
    crate::operations::commit();
    Ok(info)
}
fn full_image(directory: &Path, id: &str) -> Result<(Vec<u8>, &'static str, DynamicImage), String> {
    let bytes = read_bytes(&contained_file(directory, id, false)?, IMAGE_BYTES)?;
    if hash(&bytes) != id {
        return Err(
            "A imagem armazenada não corresponde à referência; o arquivo foi preservado.".into(),
        );
    }
    let (mime, decoded) = inspect(&bytes)?;
    Ok((bytes, mime, decoded))
}
fn read_at(root: &Path, id: &str, thumbnail: bool) -> Result<ImageData, String> {
    let directory = image_dir(root)?;
    if thumbnail {
        let path = contained_file(&directory, id, true)?;
        if path.exists() {
            let bytes = read_bytes(&path, 2 * 1024 * 1024)?;
            let (mime, decoded) = inspect(&bytes)?;
            if mime != "image/png" || decoded.width() > 480 || decoded.height() > 480 {
                return Err("Miniatura inválida; o original foi preservado.".into());
            }
            return Ok(ImageData {
                data_url: format!("data:image/png;base64,{}", STANDARD.encode(bytes)),
            });
        }
    }
    let (mut bytes, mut mime, decoded) = full_image(&directory, id)?;
    if thumbnail {
        let preview = decoded.thumbnail(480, 480);
        bytes.clear();
        preview
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        mime = "image/png";
        persist(&directory, id, &bytes, true)?;
    }
    crate::operations::check()?;
    Ok(ImageData {
        data_url: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
    })
}
#[tauri::command]
pub async fn case_image_add(base64: String, name: String) -> Result<ImageInfo, String> {
    crate::offload(move || {
        let _guard = IMAGE_LOCK.lock();
        add_at(&crate::config_dir(), &base64, &name)
    })
    .await?
}
#[tauri::command]
pub async fn case_image_read(id: String, thumbnail: Option<bool>) -> Result<ImageData, String> {
    crate::offload(move || {
        let _guard = IMAGE_LOCK.lock();
        read_at(&crate::config_dir(), &id, thumbnail.unwrap_or(false))
    })
    .await?
}

fn attachments<'a>(data: &'a Value) -> Result<Vec<&'a Value>, String> {
    attachments_from(
        data.get("cases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    )
}
fn attachments_from<'a>(
    cases: impl IntoIterator<Item = &'a Value>,
) -> Result<Vec<&'a Value>, String> {
    let mut result = Vec::new();
    for case in cases {
        for container in ["items", "caseTrails"] {
            for item in case
                .get(container)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(images) = item.get("attachments").filter(|v| !v.is_null()) {
                    let images = images
                        .as_array()
                        .ok_or("Lista de imagens do Caso inválida.")?;
                    if result.len().saturating_add(images.len()) > MAX_IMAGES {
                        return Err(
                            "A investigação aceita até 1.000 referências de imagens.".into()
                        );
                    }
                    result.extend(images);
                }
            }
        }
    }
    Ok(result)
}
pub(crate) fn references(data: &Value) -> Result<BTreeMap<String, String>, String> {
    reference_list(attachments(data)?)
}
pub(crate) fn references_from_cases<'a>(
    cases: impl IntoIterator<Item = &'a Value>,
) -> Result<BTreeMap<String, String>, String> {
    reference_list(attachments_from(cases)?)
}
fn reference_list(attachments: Vec<&Value>) -> Result<BTreeMap<String, String>, String> {
    let mut refs = BTreeMap::new();
    for attachment in attachments {
        let object = attachment
            .as_object()
            .ok_or("Referência de imagem inválida.")?;
        if ["base64", "dataUrl", "data_url"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return Err("Salve apenas referências de imagens no Caso; importe o conteúdo com case_image_add.".into());
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Imagem sem identificador.")?;
        valid_id(id)?;
        refs.entry(id.into()).or_insert_with(|| {
            name(
                object
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Imagem"),
            )
        });
    }
    Ok(refs)
}
fn normalize_references(data: &mut Value, verified: &BTreeMap<String, ImageInfo>) {
    for case in data
        .get_mut("cases")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        for container in ["items", "caseTrails"] {
            for item in case
                .get_mut(container)
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                for attachment in item
                    .get_mut("attachments")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    let Some(object) = attachment.as_object_mut() else {
                        continue;
                    };
                    let Some(info) = object
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|id| verified.get(id))
                    else {
                        continue;
                    };
                    let filename = name(
                        object
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("Imagem"),
                    );
                    object.insert("name".into(), Value::String(filename));
                    object.insert("mime".into(), Value::String(info.mime.clone()));
                    object.insert("bytes".into(), Value::from(info.bytes));
                    object.insert("width".into(), Value::from(info.width));
                    object.insert("height".into(), Value::from(info.height));
                }
            }
        }
    }
}

/// Validate one image at a time; archive payloads never become a giant base64
/// JSON value. Files below were generated by the bounded archive reader.
pub(crate) fn validate_portable_images(
    data: &mut Value,
    files: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let refs = references(data)?;
    if refs.len() != files.len() || refs.keys().any(|id| !files.contains_key(id)) {
        return Err(
            "As imagens do arquivo portátil não correspondem às referências do Caso.".into(),
        );
    }
    let mut verified = BTreeMap::new();
    for (id, path) in files {
        let bytes = read_bytes(path, IMAGE_BYTES)?;
        if hash(&bytes) != *id {
            return Err("A integridade de uma imagem portátil não confere.".into());
        }
        let (mime, decoded) = inspect(&bytes)?;
        verified.insert(
            id.clone(),
            ImageInfo {
                id: id.clone(),
                name: String::new(),
                mime: mime.into(),
                bytes: bytes.len(),
                width: decoded.width(),
                height: decoded.height(),
            },
        );
    }
    normalize_references(data, &verified);
    Ok(())
}

pub(crate) fn publish_portable_images(
    root: &Path,
    files: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let directory = image_dir(root)?;
    for (id, path) in files {
        let bytes = read_bytes(path, IMAGE_BYTES)?;
        if hash(&bytes) != *id {
            return Err("Uma imagem portátil mudou após a validação.".into());
        }
        persist(&directory, id, &bytes, false)?;
    }
    Ok(())
}

pub(crate) fn portable_image_sources(
    root: &Path,
    data: &Value,
) -> Result<Vec<crate::case_archive_format::Source>, String> {
    let refs = references(data)?;
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let directory = image_dir(root)?;
    let mut sources = Vec::new();
    for id in refs.keys() {
        let (bytes, _, decoded) = full_image(&directory, id)?;
        drop(decoded);
        let size = bytes.len() as u64;
        drop(bytes);
        sources.push(crate::case_archive_format::Source {
            entry: crate::case_archive_format::Entry {
                kind: crate::case_archive_format::EntryKind::Image(id.clone()),
                bytes: size,
                sha256: id.clone(),
            },
            file: fs::File::open(contained_file(&directory, id, false)?)
                .map_err(|e| e.to_string())?,
        });
    }
    Ok(sources)
}

/// Native portable callers charge encoded/decoded ownership before image
/// allocation. Each image is released before the next one is inspected.
fn native_inspect(
    bytes: &[u8],
    credit: &mut crate::case_work_budget::Lease,
) -> Result<ImageInfo, String> {
    let format = image::guess_format(bytes).map_err(|_| "Formato de imagem não reconhecido.")?;
    credit.merge(crate::case_cache::reserve_work(credit.pool(), 4 << 20)?)?;
    let mut header = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_000);
    limits.max_image_height = Some(16_000);
    limits.max_alloc = Some(4 << 20);
    header.limits(limits);
    let (width, height) = header
        .into_dimensions()
        .map_err(|_| "Cabeçalho de imagem inválido.")?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|pixels| *pixels <= IMAGE_PIXELS)
        .ok_or("A imagem excede 16 megapixels.")?;
    let decoded = usize::try_from(pixels)
        .map_err(|_| "Imagem excessiva.")?
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(64 << 10))
        .ok_or("Imagem excessiva.")?;
    credit.merge(crate::case_cache::reserve_work(credit.pool(), decoded)?)?;
    let (mime, image) = inspect(bytes)?;
    let info = ImageInfo {
        id: hash(bytes),
        name: String::new(),
        mime: mime.into(),
        bytes: bytes.len(),
        width: image.width(),
        height: image.height(),
    };
    drop(image);
    Ok(info)
}
pub(crate) fn stage_native_embedded_image(
    directory: &Path,
    ordinal: usize,
    raw: crate::case_evidence::RawJson<'_>,
) -> Result<(String, PathBuf), String> {
    let mut id = None;
    let mut encoded = None;
    let mut seen = std::collections::HashSet::new();
    let mut fields = raw.members()?;
    while let Some(field) = fields.next()? {
        if field.key.len() > 4096 {
            return Err("Campo de imagem excessivo.".into());
        }
        let key = field.key()?;
        if !seen.insert(key.clone()) {
            return Err("Campo de imagem duplicado.".into());
        }
        match key.as_str() {
            "id" => {
                if field.value.get().len() > 128 {
                    return Err("Identificador de imagem inválido.".into());
                }
                let value: String = serde_json::from_str(field.value.get())
                    .map_err(|_| "Identificador de imagem inválido.")?;
                valid_id(&value)?;
                id = Some(value);
            }
            "base64" => encoded = Some(field.value),
            _ => (),
        }
    }
    let id = id.ok_or("Imagem incorporada sem identificador.")?;
    let encoded = encoded.ok_or("Imagem incorporada sem conteúdo.")?;
    if encoded.get().len() > IMAGE_BYTES.div_ceil(3) * 8 + 2 {
        return Err("Imagem incorporada excessiva.".into());
    }
    let decoded_bound = encoded
        .get()
        .len()
        .div_ceil(4)
        .saturating_mul(3)
        .min(IMAGE_BYTES);
    let mut credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        encoded
            .get()
            .len()
            .checked_add(decoded_bound * 2 + (64 << 10))
            .ok_or("Imagem excessiva.")?,
    )?;
    let encoded: String =
        serde_json::from_str(encoded.get()).map_err(|_| "Base64 incorporado inválido.")?;
    let bytes = decode_base64(&encoded)?;
    if hash(&bytes) != id {
        return Err("A integridade da imagem incorporada não confere.".into());
    }
    native_inspect(&bytes, &mut credit)?;
    let path = directory.join(ordinal.to_string());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok((id, path))
}
pub(crate) fn validate_native_images(
    data: &mut Value,
    files: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let refs = references(data)?;
    if refs.len() != files.len() || refs.keys().any(|id| !files.contains_key(id)) {
        return Err("As imagens portáteis não correspondem às referências do Caso.".into());
    }
    let (verified, _credit) = native_verified_images(files)?;
    normalize_references(data, &verified);
    Ok(())
}
fn native_verified_images(
    files: &BTreeMap<String, PathBuf>,
) -> Result<(BTreeMap<String, ImageInfo>, crate::case_work_budget::Lease), String> {
    let credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        files.len().checked_mul(2048).ok_or("Imagens excessivas.")?,
    )?;
    let mut verified = BTreeMap::new();
    for (id, path) in files {
        let bytes = fs::metadata(path).map_err(|e| e.to_string())?.len();
        if bytes > IMAGE_BYTES as u64 {
            return Err("Imagem excessiva.".into());
        }
        let mut credit = crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            bytes as usize * 2 + (64 << 10),
        )?;
        let bytes = read_bytes(path, IMAGE_BYTES)?;
        if hash(&bytes) != *id {
            return Err("A integridade da imagem portátil não confere.".into());
        }
        verified.insert(id.clone(), native_inspect(&bytes, &mut credit)?);
    }
    Ok((verified, credit))
}
pub(crate) fn publish_native_images(
    root: &Path,
    files: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let directory = image_dir(root)?;
    for (id, path) in files {
        let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > IMAGE_BYTES as u64 {
            return Err("Imagem excessiva.".into());
        }
        let _credit = crate::case_cache::reserve_work(
            crate::case_work_budget::global(),
            size as usize * 4 + (64 << 10),
        )?;
        let bytes = read_bytes(path, IMAGE_BYTES)?;
        if hash(&bytes) != *id {
            return Err("A imagem mudou após a preparação.".into());
        }
        persist(&directory, id, &bytes, false)?;
    }
    Ok(())
}
pub(crate) fn native_image_sources(
    root: &Path,
    data: &Value,
) -> Result<
    (
        Vec<crate::case_archive_format::Source>,
        crate::case_work_budget::Lease,
    ),
    String,
> {
    let refs = references(data)?;
    native_image_sources_from_refs(root, &refs)
}
pub(crate) fn native_image_sources_from_refs(
    root: &Path,
    refs: &BTreeMap<String, String>,
) -> Result<
    (
        Vec<crate::case_archive_format::Source>,
        crate::case_work_budget::Lease,
    ),
    String,
> {
    let credit = crate::case_cache::reserve_work(
        crate::case_work_budget::global(),
        refs.len().checked_mul(2048).ok_or("Imagens excessivas.")?,
    )?;
    if refs.is_empty() {
        return Ok((Vec::new(), credit));
    }
    let directory = image_dir(root)?;
    let paths = refs
        .keys()
        .map(|id| Ok((id.clone(), contained_file(&directory, id, false)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    native_verified_images(&paths)?;
    let sources = paths
        .into_iter()
        .map(|(id, path)| {
            let file = crate::case_archive_format::open_regular(&path)?;
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            Ok(crate::case_archive_format::Source {
                entry: crate::case_archive_format::Entry {
                    kind: crate::case_archive_format::EntryKind::Image(id.clone()),
                    bytes: size,
                    sha256: id,
                },
                file,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((sources, credit))
}

pub(crate) fn validate_document(data: &Value) -> Result<(), String> {
    if data
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        > 2
    {
        return Err("Esta investigação exige uma versão mais recente.".into());
    }
    let cases = data
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("Arquivo sem investigações.")?;
    if cases.is_empty()
        || cases.iter().any(|c| {
            !c.is_object()
                || !c
                    .get("id")
                    .is_some_and(|v| v.is_string() && !v.as_str().unwrap().is_empty())
        })
    {
        return Err("Estrutura de investigação inválida.".into());
    }
    references(data)?;
    Ok(())
}
struct LimitedWriter<W> {
    inner: W,
    written: usize,
}
impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // write_all retries Interrupted indefinitely. Cancellation must be final.
        if crate::operations::cancelled() {
            return Err(std::io::Error::other("Operação cancelada."));
        }
        if self.written.saturating_add(bytes.len()) > DOCUMENT_BYTES {
            return Err(std::io::Error::other(
                "A investigação com imagens excede 64 MiB.",
            ));
        }
        let count = self.inner.write(bytes)?;
        self.written += count;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn export_at(root: &Path, path: &Path, mut data: Value, mask: bool) -> Result<(), String> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("licase"))
    {
        return crate::case_archive::export_at(root, path, data, mask);
    }
    validate_document(&data)?;
    // Notes may be ahead of an autosave, but context/visibility never come from
    // the UI document. Capture all contexts from one authoritative read view.
    let legacy_ids = refresh_export_contexts(root, &mut data)?;
    validate_json_contexts(&data)?;
    if mask {
        crate::workspace::redact_value(&mut data);
    }
    let refs = references(&data)?;
    let directory = image_dir(root)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut writer = LimitedWriter {
            inner: std::io::BufWriter::new(temp.as_file_mut()),
            written: 0,
        };
        writer.write_all(b"{").map_err(|e| e.to_string())?;
        let mut first = true;
        for (key, value) in data.as_object().ok_or("Investigação inválida.")? {
            if key == "imageAssets" {
                continue;
            }
            if !first {
                writer.write_all(b",").map_err(|e| e.to_string())?;
            }
            first = false;
            serde_json::to_writer(&mut writer, key).map_err(|e| e.to_string())?;
            writer.write_all(b":").map_err(|e| e.to_string())?;
            serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
        }
        if !first {
            writer.write_all(b",").map_err(|e| e.to_string())?;
        }
        writer
            .write_all(b"\"imageAssets\":[")
            .map_err(|e| e.to_string())?;
        for (i, (id, filename)) in refs.iter().enumerate() {
            let (bytes, _, decoded) = full_image(&directory, id)?;
            drop(decoded);
            if i > 0 {
                writer.write_all(b",").map_err(|e| e.to_string())?;
            }
            let asset =
                serde_json::json!({"id":id,"name":filename,"base64":STANDARD.encode(bytes)});
            serde_json::to_writer(&mut writer, &asset).map_err(|e| e.to_string())?;
        }
        writer
            .write_all(b"]}")
            .and_then(|_| writer.flush())
            .map_err(|e| e.to_string())?;
    }
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    crate::case_archive::recheck_legacy_export(root, &legacy_ids)?;
    crate::operations::check()?;
    temp.persist(path).map_err(|e| e.error.to_string())?;
    crate::operations::commit();
    Ok(())
}
pub(crate) fn import_at(root: &Path, path: &Path) -> Result<Value, String> {
    if crate::case_archive_format::is_archive(path)? {
        return crate::case_archive::import_at(root, path);
    }
    let bytes = read_bytes(path, DOCUMENT_BYTES)?;
    let mut data: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Investigação inválida: {e}"))?;
    drop(bytes);
    validate_document(&data)?;
    // Validate before publishing even a single image. Plain JSON does not carry
    // immutable exclusion payloads; resetting a foreign revision would expose
    // records the exported investigation deliberately excluded.
    validate_json_contexts(&data)?;
    let refs = references(&data)?;
    let assets = data
        .as_object_mut()
        .and_then(|object| object.remove("imageAssets"))
        .unwrap_or_else(|| Value::Array(vec![]));
    let assets = assets
        .as_array()
        .ok_or("Lista de imagens incorporadas inválida.")?;
    if assets.len() > MAX_IMAGES {
        return Err("O arquivo contém mais de 1.000 imagens.".into());
    }
    let directory = image_dir(root)?;
    let mut validated = BTreeMap::new();
    let mut verified = BTreeMap::new();
    // Validate every asset and every reference before publishing any new image.
    for asset in assets {
        crate::operations::check()?;
        let id = asset
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Imagem incorporada sem identificador.")?;
        valid_id(id)?;
        let encoded = asset
            .get("base64")
            .and_then(Value::as_str)
            .ok_or("Imagem incorporada sem conteúdo.")?;
        let bytes = decode_base64(encoded)?;
        if hash(&bytes) != id {
            return Err(
                "O conteúdo de uma imagem incorporada não corresponde ao identificador.".into(),
            );
        }
        let (mime, decoded) = inspect(&bytes)?;
        if refs.contains_key(id) {
            verified.insert(
                id.to_owned(),
                ImageInfo {
                    id: id.to_owned(),
                    name: String::new(),
                    mime: mime.into(),
                    bytes: bytes.len(),
                    width: decoded.width(),
                    height: decoded.height(),
                },
            );
            validated.insert(id.to_owned(), bytes);
        }
        drop(decoded);
    }
    for id in refs.keys() {
        let path = contained_file(&directory, id, false)?;
        if path.exists() || !validated.contains_key(id) {
            let (bytes, mime, decoded) = full_image(&directory, id)?;
            verified.insert(
                id.to_owned(),
                ImageInfo {
                    id: id.to_owned(),
                    name: String::new(),
                    mime: mime.into(),
                    bytes: bytes.len(),
                    width: decoded.width(),
                    height: decoded.height(),
                },
            );
            drop(decoded);
        }
    }
    normalize_references(&mut data, &verified);
    for (id, bytes) in validated {
        persist(&directory, &id, &bytes, false)?;
    }
    if !refs.is_empty() {
        crate::operations::commit();
    }
    Ok(data)
}

fn refresh_export_contexts(root: &Path, data: &mut Value) -> Result<Vec<String>, String> {
    let conn = crate::case_store::context_connection(root)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let legacy_ids = crate::case_archive::require_legacy_export(&conn, data)?;
    for case in data["cases"]
        .as_array_mut()
        .ok_or("Arquivo sem investigações.")?
    {
        crate::analysis_context::attach(&conn, case).map_err(|error| {
            format!("Não foi possível exportar a configuração atual do Caso. Salve e reabra o Caso antes de tentar novamente: {error}")
        })?;
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(legacy_ids)
}

fn validate_json_contexts(data: &Value) -> Result<(), String> {
    for case in data["cases"]
        .as_array()
        .ok_or("Arquivo sem investigações.")?
    {
        let Some(value) = case.get("analysisContext") else {
            continue; // Legacy JSON has no Case-owned context.
        };
        let context: crate::analysis_context::Snapshot = serde_json::from_value(value.clone())
            .map_err(|_| {
                "Configuração do Caso inválida; o arquivo original foi preservado.".to_string()
            })?;
        if context.visibility_revision != 0 {
            return Err("O formato JSON não transporta o histórico de exclusões deste Caso. Exporte o Caso original como investigação portátil (.licase) e importe esse arquivo. A operação foi interrompida sem alterar seus arquivos.".into());
        }
        if context.schema_version != 1
            || Some(context.case_id.as_str()) != case.get("id").and_then(Value::as_str)
            || uuid::Uuid::parse_str(&context.analysis_id).is_err()
        {
            return Err("Identidade da configuração do Caso inválida ou não suportada.".into());
        }
        crate::analysis_context::validate(&context.config)?;
    }
    Ok(())
}
#[tauri::command]
pub async fn export_investigation(
    path: String,
    data: Value,
    mask: Option<bool>,
) -> Result<(), String> {
    crate::offload(move || {
        let _guard = IMAGE_LOCK.lock();
        export_at(
            &crate::config_dir(),
            Path::new(&path),
            data,
            mask.unwrap_or(false),
        )
    })
    .await?
}
pub(crate) fn import_document(path: &str) -> Result<Value, String> {
    let _guard = IMAGE_LOCK.lock();
    import_at(&crate::config_dir(), Path::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::new_rgb8(width, height);
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }
    fn document(image: &ImageInfo) -> Value {
        json!({"schemaVersion":2,"activeCaseId":"case-test","cases":[{
            "id":"case-test","title":"Investigação sintética",
            "items":[{"id":"item-test","note":"Nota legada","summary":"Resumo","details":"Detalhes","attachments":[image]}],
            "caseTrails":[{"id":"trail-test","title":"Trilha manual","itemIds":["item-test"],"attachments":[image]}]
        }]})
    }
    fn write_json(path: &Path, value: &Value) {
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn save_document(root: &Path, data: Value) -> Value {
        crate::case_store::save_at(root, data).unwrap();
        let mut saved = crate::case_store::load_at(root).unwrap();
        saved.as_object_mut().unwrap().remove("revision");
        saved
    }

    #[test]
    fn case_images_accept_supported_static_formats_and_deduplicate() {
        let root = tempfile::tempdir().unwrap();
        for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
            let bytes = fixture(format, 2, 3);
            let added = add_at(
                root.path(),
                &STANDARD.encode(&bytes),
                r"C:\prints\evidence.png",
            )
            .unwrap();
            assert_eq!((added.width, added.height), (2, 3));
            assert_eq!(added.name, "evidence.png");
            assert_eq!(added.id, hash(&bytes));
            assert_eq!(added.bytes, bytes.len());
            let duplicate = add_at(root.path(), &STANDARD.encode(&bytes), "Second name").unwrap();
            assert_eq!(duplicate.id, added.id);
            let data = read_at(root.path(), &added.id, false).unwrap().data_url;
            assert!(data.starts_with(&format!("data:{};base64,", added.mime)));
            assert_eq!(
                STANDARD.decode(data.split_once(',').unwrap().1).unwrap(),
                bytes
            );
        }
        assert_eq!(
            fs::read_dir(image_dir(root.path()).unwrap())
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    fn case_images_thumbnail_is_small_cached_and_preserves_original() {
        let root = tempfile::tempdir().unwrap();
        let bytes = fixture(ImageFormat::Png, 1024, 256);
        let added = add_at(root.path(), &STANDARD.encode(&bytes), "wide.png").unwrap();
        let first = read_at(root.path(), &added.id, true).unwrap().data_url;
        let second = read_at(root.path(), &added.id, true).unwrap().data_url;
        assert_eq!(first, second);
        let thumbnail = STANDARD.decode(first.split_once(',').unwrap().1).unwrap();
        let (_, decoded) = inspect(&thumbnail).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (480, 120));
        assert_eq!(
            fs::read(root.path().join("case-images").join(added.id)).unwrap(),
            bytes
        );
    }

    #[test]
    fn case_images_reject_invalid_content_ids_dimensions_and_inline_data() {
        let root = tempfile::tempdir().unwrap();
        for invalid in ["../outside", "", &"A".repeat(64), &"x".repeat(64)] {
            assert!(read_at(root.path(), invalid, false).is_err());
        }
        assert!(add_at(root.path(), "not base64!", "x").is_err());
        assert!(add_at(
            root.path(),
            &STANDARD.encode(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            "x.svg"
        )
        .is_err());
        assert!(decode_base64(&"A".repeat(IMAGE_BYTES.div_ceil(3) * 4 + 1)).is_err());
        let png = fixture(ImageFormat::Png, 1, 1);
        assert!(inspect(&png[..png.len() - 1]).is_err());
        let mut appended = png.clone();
        appended.extend_from_slice(b"trailing data");
        assert!(inspect(&appended).is_err());
        let mut oversized = png.clone();
        oversized[16..20].copy_from_slice(&16_001u32.to_be_bytes());
        // Correct the IHDR CRC so the dimensional rejection is independent of corruption.
        let mut crc = !0u32;
        for &byte in &oversized[12..29] {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb8_8320 } else { 0 };
            }
        }
        oversized[29..33].copy_from_slice(&(!crc).to_be_bytes());
        assert!(inspect(&oversized).is_err());
        let added = add_at(root.path(), &STANDARD.encode(png), "x").unwrap();
        let mut data = document(&added);
        data["cases"][0]["items"][0]["attachments"][0]["base64"] = json!("AA==");
        assert!(references(&data).is_err());
        let mut jpeg = fixture(ImageFormat::Jpeg, 1, 1);
        jpeg.pop();
        assert!(inspect(&jpeg).is_err());
    }

    #[test]
    fn case_images_portable_round_trip_deduplicates_assets_and_preserves_fields() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let bytes = fixture(ImageFormat::Png, 2, 2);
        let added = add_at(source.path(), &STANDARD.encode(&bytes), "Print.png").unwrap();
        let data = save_document(source.path(), document(&added));
        let path = source.path().join("investigation.json");
        export_at(source.path(), &path, data.clone(), false).unwrap();
        let exported: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(exported["imageAssets"].as_array().unwrap().len(), 1);
        assert!(data["cases"][0]["items"][0]["attachments"][0]
            .get("base64")
            .is_none());
        let imported = import_at(destination.path(), &path).unwrap();
        assert_eq!(imported, data);
        assert!(imported.get("imageAssets").is_none());
        assert_eq!(
            fs::read(destination.path().join("case-images").join(&added.id)).unwrap(),
            bytes
        );
        assert_eq!(import_at(destination.path(), &path).unwrap(), data);
        assert_eq!(
            fs::read_dir(destination.path().join("case-images"))
                .unwrap()
                .count(),
            1
        );
        let mut forged = exported;
        forged["cases"][0]["items"][0]["attachments"][0]["mime"] = json!("image/svg+xml");
        forged["cases"][0]["items"][0]["attachments"][0]["width"] = json!(0);
        forged["cases"][0]["items"][0]["attachments"][0]["bytes"] = json!(-1);
        write_json(&path, &forged);
        assert_eq!(import_at(destination.path(), &path).unwrap(), data);
        let legacy = json!({"schemaVersion":1,"cases":[{"id":"legacy","items":[{"note":"old"}]}]});
        write_json(&path, &legacy);
        assert_eq!(import_at(destination.path(), &path).unwrap(), legacy);
    }

    #[test]
    fn legacy_export_refuses_native_adoption_during_serialization() {
        for extension in ["json", "licase"] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let data = save_document(
                root.path(),
                json!({"schemaVersion":2,"cases":[{"id":"case","items":[{"id":"item","rows":[{"id":1,"future":1.0}]}]}]}),
            );
            let path = outside.path().join(format!("existing.{extension}"));
            fs::write(&path, b"original export").unwrap();
            let native_root = root.path().to_path_buf();
            crate::case_archive::BEFORE_LEGACY_EXPORT_RECHECK.with(|hook|*hook.borrow_mut()=Some(Box::new(move|| {
                let conn=crate::case_store::connect(&native_root).unwrap();
                conn.execute_batch("CREATE TABLE native_evidence_protected(case_id TEXT PRIMARY KEY,store_id TEXT NOT NULL,analysis_id TEXT NOT NULL);INSERT INTO native_evidence_protected VALUES('case','native-store','native-analysis');").unwrap();
            })));
            let error = export_at(root.path(), &path, data, false).unwrap_err();
            assert!(error.starts_with("CASE_NATIVE_EXPORT_REQUIRED"), "{error}");
            assert_eq!(fs::read(&path).unwrap(), b"original export");
        }
    }

    #[test]
    fn legacy_exports_refuse_restore_barrier_before_capture_and_publication() {
        fn protect_restored_case(root: &Path) {
            let conn = crate::case_store::connect(root).unwrap();
            let native_tables: u32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name IN ('native_evidence_protected','native_evidence_cases')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                native_tables, 0,
                "restore protection must work before native adoption"
            );
            conn.execute_batch(
                "CREATE TABLE case_recovery_protected(case_id TEXT PRIMARY KEY);INSERT INTO case_recovery_protected VALUES('case');",
            )
            .unwrap();
        }

        for extension in ["json", "licase"] {
            for before_capture in [true, false] {
                let root = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let mut data = save_document(
                    root.path(),
                    json!({"schemaVersion":2,"cases":[{"id":"case","items":[{"id":"item","rows":[{"id":1,"future":18446744073709551615u64}]}]}]}),
                );
                data["cases"][0]["items"][0]["rows"] =
                    json!([{"id":1,"future":18446744073709551616.0,"fabricated":true}]);
                let path = outside.path().join(format!("existing.{extension}"));
                fs::write(&path, b"original export").unwrap();
                if before_capture {
                    protect_restored_case(root.path());
                } else {
                    let restored_root = root.path().to_path_buf();
                    crate::case_archive::BEFORE_LEGACY_EXPORT_RECHECK.with(|hook| {
                        *hook.borrow_mut() =
                            Some(Box::new(move || protect_restored_case(&restored_root)));
                    });
                }
                let error = export_at(root.path(), &path, data, false).unwrap_err();
                assert!(error.starts_with("CASE_NATIVE_EXPORT_REQUIRED"), "{error}");
                assert_eq!(fs::read(&path).unwrap(), b"original export");
            }
        }
    }

    #[test]
    fn legacy_exports_refuse_protected_case_records_and_preserve_destination() {
        let legacy = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        let data = save_document(
            legacy.path(),
            json!({"schemaVersion":2,"active":"case","cases":[{"id":"case","items":[{"id":"i","rows":[{"id":1,"future":18446744073709551615u64}]}]}]}),
        );
        for extension in ["json", "licase"] {
            export_at(
                legacy.path(),
                &exports.path().join(format!("legacy.{extension}")),
                data.clone(),
                false,
            )
            .unwrap();
        }
        let conn = crate::case_store::connect(legacy.path()).unwrap();
        conn.execute_batch("CREATE TABLE native_evidence_protected(case_id TEXT PRIMARY KEY,store_id TEXT NOT NULL,analysis_id TEXT NOT NULL);INSERT INTO native_evidence_protected VALUES('case','native-store','native-analysis');").unwrap();
        drop(conn);
        let mut forged = data;
        forged["cases"][0]["items"][0]["rows"] =
            json!([{"id":1,"future":18446744073709551616.0,"fabricated":true}]);
        for extension in ["json", "licase"] {
            let path = exports.path().join(format!("protected.{extension}"));
            fs::write(&path, b"preserved destination").unwrap();
            let error = export_at(legacy.path(), &path, forged.clone(), false).unwrap_err();
            assert!(error.starts_with("CASE_NATIVE_EXPORT_REQUIRED"), "{error}");
            assert_eq!(fs::read(&path).unwrap(), b"preserved destination");
        }
    }

    #[test]
    fn case_images_export_masks_metadata_without_changing_image_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("masked.json");
        let bytes = fixture(ImageFormat::Png, 1, 1);
        let added = add_at(
            root.path(),
            &STANDARD.encode(&bytes),
            "password=synthetic-example",
        )
        .unwrap();
        let mut data = document(&added);
        data["cases"][0]["token"] = json!("synthetic-secret");
        data["cases"][0]["items"][0]["details"] = json!("password=synthetic-detail");
        let data = save_document(root.path(), data);
        export_at(root.path(), &path, data, true).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert!(!text.contains("synthetic-example"));
        assert!(!text.contains("synthetic-secret"));
        assert!(!text.contains("synthetic-detail"));
        let exported: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(exported["cases"][0]["token"], "[oculto]");
        assert_eq!(
            STANDARD
                .decode(exported["imageAssets"][0]["base64"].as_str().unwrap())
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn case_images_bad_import_and_missing_export_preserve_existing_files() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let bytes = fixture(ImageFormat::Png, 2, 2);
        let added = add_at(source.path(), &STANDARD.encode(&bytes), "valid").unwrap();
        let other = fixture(ImageFormat::Png, 3, 3);
        let original = add_at(destination.path(), &STANDARD.encode(&other), "original").unwrap();
        let path = source.path().join("bad.json");
        let mut data = document(&added);
        data["imageAssets"] = json!([
            {"id":added.id,"base64":STANDARD.encode(&bytes)},
            {"id":"0".repeat(64),"base64":STANDARD.encode(&bytes)}
        ]);
        write_json(&path, &data);
        assert!(import_at(destination.path(), &path).is_err());
        assert!(!destination
            .path()
            .join("case-images")
            .join(&added.id)
            .exists());
        assert_eq!(
            fs::read(destination.path().join("case-images").join(original.id)).unwrap(),
            other
        );
        let existing = destination.path().join("existing.json");
        fs::write(&existing, b"old document").unwrap();
        assert!(export_at(destination.path(), &existing, document(&added), false).is_err());
        assert_eq!(fs::read(existing).unwrap(), b"old document");
        write_json(&path, &document(&added));
        assert!(import_at(destination.path(), &path).is_err());
        fs::write(
            source.path().join("case-images").join(&added.id),
            b"corrupted",
        )
        .unwrap();
        assert!(read_at(source.path(), &added.id, false).is_err());
        assert!(add_at(source.path(), &STANDARD.encode(&bytes), "valid").is_err());
        assert_eq!(
            fs::read(source.path().join("case-images").join(added.id)).unwrap(),
            b"corrupted"
        );
    }

    #[test]
    fn case_images_writer_size_and_cancellation_are_terminal_errors() {
        let mut full = LimitedWriter {
            inner: Vec::<u8>::new(),
            written: DOCUMENT_BYTES,
        };
        assert!(full.write_all(b"x").is_err());
        assert!(full.inner.is_empty());
        let generation = crate::operations::generation();
        let result = crate::operations::run(generation, || {
            crate::operations::cancel();
            let mut writer = LimitedWriter {
                inner: Vec::<u8>::new(),
                written: 0,
            };
            let error = writer.write(b"x").unwrap_err();
            assert_ne!(error.kind(), std::io::ErrorKind::Interrupted);
            assert!(writer.inner.is_empty());
        });
        assert!(result.is_err());
    }

    #[test]
    fn case_images_export_refreshes_context_and_fails_closed_for_exclusions() {
        let root = tempfile::tempdir().unwrap();
        let original = save_document(root.path(), json!({"cases":[{"id":"case-test"}]}));
        let mut stale = original.clone();
        stale["cases"][0]["analysisContext"] = json!({"stale":"untrusted UI"});
        let path = root.path().join("case.json");
        export_at(root.path(), &path, stale.clone(), false).unwrap();
        let exported: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            exported["cases"][0]["analysisContext"],
            original["cases"][0]["analysisContext"]
        );

        let identity = serde_json::from_value::<crate::analysis_context::Snapshot>(
            original["cases"][0]["analysisContext"].clone(),
        )
        .unwrap()
        .identity();
        crate::analysis_context::visibility_update_at(
            root.path(),
            &identity,
            &|| false,
            |_| Ok(()),
        )
        .unwrap();
        let previous = fs::read(&path).unwrap();
        let error = export_at(root.path(), &path, stale, false).unwrap_err();
        assert!(error.contains("exclusões"));
        assert_eq!(fs::read(path).unwrap(), previous);
    }

    #[test]
    fn case_images_exclusion_import_fails_before_any_image_publication() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let bytes = fixture(ImageFormat::Png, 1, 1);
        let added = add_at(source.path(), &STANDARD.encode(&bytes), "image").unwrap();
        let mut data = save_document(source.path(), document(&added));
        data["cases"][0]["analysisContext"]["visibilityRevision"] = json!(3);
        data["imageAssets"] = json!([{ "id":added.id, "base64":STANDARD.encode(&bytes) }]);
        let path = source.path().join("foreign.json");
        write_json(&path, &data);
        let before = fs::read(&path).unwrap();
        assert!(import_at(target.path(), &path)
            .unwrap_err()
            .contains("exclusões"));
        assert!(!target.path().join("case-images").exists());
        assert_eq!(fs::read(&path).unwrap(), before);
        data["cases"][0]["analysisContext"]["visibilityRevision"] = json!("unknown");
        write_json(&path, &data);
        assert!(import_at(target.path(), &path).is_err());
        assert!(!target.path().join("case-images").exists());
    }

    #[test]
    fn case_images_portable_binary_assets_validate_before_publication() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let bytes = fixture(ImageFormat::Png, 2, 3);
        let info = add_at(source.path(), &STANDARD.encode(&bytes), "Image.png").unwrap();
        let mut data = document(&info);
        let file = source.path().join("case-images").join(&info.id);
        let files = BTreeMap::from([(info.id.clone(), file)]);
        assert!(validate_portable_images(&mut data, &BTreeMap::new()).is_err());
        data["cases"][0]["items"][0]["attachments"][0]["width"] = json!(100);
        validate_portable_images(&mut data, &files).unwrap();
        assert_eq!(data["cases"][0]["items"][0]["attachments"][0]["width"], 2);
        assert!(!target.path().join("case-images").exists());
        publish_portable_images(target.path(), &files).unwrap();
        assert_eq!(
            fs::read(target.path().join("case-images").join(&info.id)).unwrap(),
            bytes
        );
        let sources = portable_image_sources(target.path(), &data).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].entry.bytes, bytes.len() as u64);
        assert_eq!(sources[0].entry.sha256, info.id);
    }

    #[cfg(unix)]
    #[test]
    fn case_images_reject_symlink_storage_and_image_paths() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("case-images")).unwrap();
        assert!(image_dir(root.path()).is_err());
        fs::remove_file(root.path().join("case-images")).unwrap();
        let directory = image_dir(root.path()).unwrap();
        let id = "a".repeat(64);
        let target = outside.path().join("original");
        fs::write(&target, b"preserved").unwrap();
        symlink(&target, directory.join(&id)).unwrap();
        assert!(contained_file(&directory, &id, false).is_err());
        assert_eq!(fs::read(target).unwrap(), b"preserved");
    }
}
