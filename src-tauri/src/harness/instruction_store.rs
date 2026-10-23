use std::path::Path;

use sha2::{Digest, Sha256};

use super::{
    claude::{
        memories::{MemoryCatalog, MemoryFile, MemorySource},
        skills,
    },
    memory_links::without_references,
    Harness, ResourceKind,
};
use crate::ledger;

const STALE: &str =
    "Instruction file changed since this editor opened. Refresh the catalog and try again.";

pub(crate) fn edit(
    harness: &impl Harness,
    file: &MemoryFile,
    expected: &str,
    content: &str,
    action: &str,
) -> Result<(), String> {
    if !file.editable {
        return Err("instruction file is read-only".into());
    }
    let path = Path::new(&file.path);
    skills::ensure_plain_components(path)?;
    skills::ensure_plain_file_if_present(path)?;
    ledger::write_text(
        harness.config_root(),
        resource_kind(file.source),
        &file.id,
        action,
        path,
        expected,
        content,
    )
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("did not hold the expected state") {
            STALE.into()
        } else {
            message
        }
    })
}

pub(crate) fn delete(
    harness: &impl Harness,
    catalog: &MemoryCatalog,
    id: &str,
    expected: &str,
    edit_action: &str,
    delete_action: &str,
) -> Result<(), String> {
    let file = catalog
        .files
        .iter()
        .find(|file| file.id == id && file.editable)
        .ok_or("instruction file is no longer available for deletion")?;
    if file.content != expected {
        return Err(STALE.into());
    }
    let target = Path::new(&file.path);
    skills::ensure_plain_components(target)?;
    skills::ensure_plain_file_if_present(target)?;
    for source in catalog
        .files
        .iter()
        .filter(|source| source.id != id && source.editable)
    {
        let updated = without_references(source, target, false);
        if updated != source.content {
            edit(harness, source, &source.content, &updated, edit_action).map_err(|error| {
                format!(
                    "Could not remove references from {}. The file was not deleted. {error}",
                    source.name
                )
            })?;
        }
    }
    ledger::remove_file(
        harness.config_root(),
        resource_kind(file.source),
        id,
        delete_action,
        target,
        Sha256::digest(expected.as_bytes()).into(),
        &ledger::backup_path(harness.config_root(), id, delete_action),
    )
    .map_err(|error| error.to_string())
}

fn resource_kind(source: MemorySource) -> ResourceKind {
    match source {
        MemorySource::UserInstructions => ResourceKind::GlobalInstructions,
        _ => ResourceKind::ProjectInstructions,
    }
}
