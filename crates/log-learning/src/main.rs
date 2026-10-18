use log_learning::{Contribution, ContributionBundle, SharedModel, MAX_DOCUMENT_BYTES};
use std::{fs, io::Read, path::Path};
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err("Documento acima do orçamento".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}
fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, path] if command == "check" => {
            let model: SharedModel = read(Path::new(path))?;
            model.validate()?;
            println!(
                "{} contribuições · revisão {}",
                model.contributions.len(),
                model.revision()
            );
        }
        [command, directory, output] if command == "consolidate" => {
            let mut paths = fs::read_dir(directory)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            paths.retain(|p| p.extension().is_some_and(|v| v == "json"));
            paths.sort();
            if paths.len() > 4096 {
                return Err("Mais de 4096 contribuições".into());
            }
            let mut model = SharedModel::default();
            for path in paths {
                let value: serde_json::Value = read(&path)?;
                if value.get("contributions").is_some() {
                    let bundle: ContributionBundle =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    bundle.validate()?;
                    for contribution in &bundle.contributions {
                        model.merge(contribution)?;
                    }
                } else {
                    let contribution: Contribution =
                        serde_json::from_value(value).map_err(|e| e.to_string())?;
                    model.merge(&contribution)?;
                }
            }
            let mut bytes = serde_json::to_vec_pretty(&model).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            fs::write(output, bytes).map_err(|e| e.to_string())?;
            println!("Revisão {}", model.revision());
        }
        _ => return Err("Uso: log-learning check MODELO | consolidate DIRETÓRIO MODELO".into()),
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
