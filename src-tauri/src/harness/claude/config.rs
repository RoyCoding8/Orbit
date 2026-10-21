use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::credentials::{
    contains_mask, restore_masked, secret_name, value_shape_secret, MaskInArray, MASK,
};
use super::ClaudeCodeHarness;
use crate::harness::{atomic, Harness};

const SETTINGS_SCHEMA: &str = include_str!("../../../../schemas/settings.json");

/// Objects whose string children get secret checks (name and value shape),
/// mirroring the environment and header handling on the Servers path.
const SECRET_CONTAINERS: &[&str] = &["env", "headers"];

/// Top-level settings whose value produces credentials. Exact names, because
/// the generic substrings ("auth", "key") also occur inside native settings
/// that are not secrets (`includeCoAuthoredBy`,
/// `workflowKeywordTriggerEnabled`).
const CREDENTIAL_SETTINGS: &[&str] = &[
    "apiKeyHelper",
    "awsCredentialExport",
    "awsAuthRefresh",
    "gcpAuthRefresh",
];

struct SettingsValidator {
    validator: jsonschema::Validator,
    hook_events: std::collections::HashSet<String>,
}

static SETTINGS_VALIDATOR: OnceLock<std::result::Result<SettingsValidator, String>> =
    OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid settings schema: {0}")]
    Schema(String),
    #[error("configuration storage operation failed: {0}")]
    Storage(String),
    #[error("settings validation failed at {path}: {message}")]
    Validation { path: String, message: String },
}

pub type ConfigResult<T> = Result<T, ConfigError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsLayer {
    User,
    Project,
    Local,
}

impl SettingsLayer {
    pub(crate) fn settings_path(self, user_root: &Path, project_root: &Path) -> PathBuf {
        match self {
            Self::User => user_root.join("settings.json"),
            Self::Project => project_root.join(".claude").join("settings.json"),
            Self::Local => project_root.join(".claude").join("settings.local.json"),
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigLayer {
    pub name: String,
    /// The resolved settings file for this layer. The user layer follows
    /// `CLAUDE_CONFIG_DIR`; display must use this, not a hardcoded label.
    pub path: String,
    pub values: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveSetting {
    pub value: Value,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    pub layers: Vec<ConfigLayer>,
    pub effective: BTreeMap<String, EffectiveSetting>,
}

struct RawLayer {
    name: String,
    path: PathBuf,
    values: Map<String, Value>,
}

/// Settings layers are the only authority: user, project, and local files.
/// `.claude.json` is runtime state (prompt history, per-project state, MCP
/// credentials) and never enters the settings snapshot; the Servers page owns
/// its masked view. Every value crossing to the UI is masked by one traversal
/// ([`masked_value`]), and [`apply_value`] restores masked fields from the
/// file on disk before writing, so a masked snapshot can never replace a real
/// credential with a placeholder.
pub fn load_config(
    harness: &ClaudeCodeHarness,
    project_root: &Path,
) -> ConfigResult<ConfigSnapshot> {
    let user_root = harness.config_root();
    let layers = [
        SettingsLayer::User,
        SettingsLayer::Project,
        SettingsLayer::Local,
    ]
    .into_iter()
    .map(|layer| {
        let path = layer.settings_path(user_root, project_root);
        Ok(RawLayer {
            name: layer.label().into(),
            path: path.clone(),
            values: read_object(&path)?,
        })
    })
    .collect::<ConfigResult<Vec<_>>>()?;

    let mut effective = BTreeMap::new();
    for layer in &layers {
        for (key, value) in &layer.values {
            effective.insert(
                key.clone(),
                EffectiveSetting {
                    value: masked_value(key, value, 0, false),
                    source: layer.name.clone(),
                },
            );
        }
    }

    Ok(ConfigSnapshot {
        layers: layers
            .iter()
            .map(|layer| ConfigLayer {
                name: layer.name.clone(),
                path: layer.path.to_string_lossy().into_owned(),
                values: layer
                    .values
                    .iter()
                    .map(|(key, value)| (key.clone(), masked_value(key, value, 0, false)))
                    .collect(),
            })
            .collect(),
        effective,
    })
}

pub fn apply_value(
    harness: &ClaudeCodeHarness,
    project_root: &Path,
    layer: SettingsLayer,
    key: &str,
    value: Value,
) -> ConfigResult<()> {
    if key.trim().is_empty() {
        return Err(ConfigError::Validation {
            path: "$".into(),
            message: "setting key cannot be empty".into(),
        });
    }
    let path = layer.settings_path(harness.config_root(), project_root);
    let stored = read_object(&path)?;
    let previous = stored.get(key).cloned();
    let mut value = value;
    if let Some(previous) = &previous {
        restore_masked(previous, &mut value).map_err(|MaskInArray| ConfigError::Validation {
            path: format!("/{key}"),
            message: "a masked placeholder inside an array is not matched by position; re-enter that entry with its real value".into(),
        })?;
    }
    if contains_mask(&value) {
        return Err(ConfigError::Validation {
            path: format!("/{key}"),
            message: "value still contains a masked placeholder; enter the real credential or remove the field".into(),
        });
    }
    validate_value(key, &value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            ConfigError::Storage(format!("failed creating {}: {error}", parent.display()))
        })?;
    }
    let action = format!("config-{}", layer.label());
    let backup = crate::ledger::backup_path(harness.config_root(), key, &action);
    let patch = match previous {
        Some(previous) => atomic::Patch::ReplacePath {
            path: atomic::p![key],
            matches: previous,
            value,
        },
        None => atomic::Patch::SetPath {
            path: atomic::p![key],
            value,
        },
    };
    crate::ledger::write(
        harness.config_root(),
        crate::harness::ResourceKind::Settings,
        key,
        &action,
        &path,
        atomic::RecordedWrite::Patches(vec![patch]),
        Some(&backup),
    )
    .map_err(|error| ConfigError::Storage(error.to_string()))?;
    Ok(())
}

fn read_object(path: &Path) -> ConfigResult<Map<String, Value>> {
    let value = match atomic::read_json(path) {
        Ok(Value::Null) => Value::Object(Map::new()),
        Ok(value) => value,
        Err(error) => return Err(ConfigError::Storage(error.to_string())),
    };
    value
        .as_object()
        .cloned()
        .ok_or_else(|| ConfigError::Validation {
            path: "$".into(),
            message: format!("{} must contain a JSON object", path.display()),
        })
}

/// Validate only the edited key by presenting a single-key document to the
/// schema. Sibling keys are absent from that document, so a pre-existing
/// value the bundled schema rejects (a legacy variable name, a typed object
/// missing a new required field) can never block an unrelated edit.
fn validate_value(key: &str, value: &Value) -> ConfigResult<()> {
    let settings = settings_validator()?;
    let mut scoped = Map::new();
    scoped.insert(key.to_owned(), value.clone());
    if key == "hooks" {
        if let Some(hooks) = scoped.get_mut("hooks").and_then(Value::as_object_mut) {
            hooks.retain(|event, _| settings.hook_events.contains(event));
        }
    }
    if let Some(error) = settings
        .validator
        .iter_errors(&Value::Object(scoped))
        .next()
    {
        let path = error.instance_path.to_string();
        return Err(ConfigError::Validation {
            path: if path.is_empty() {
                format!("/{key}")
            } else {
                path
            },
            message: error.to_string(),
        });
    }
    Ok(())
}

fn settings_validator() -> ConfigResult<&'static SettingsValidator> {
    SETTINGS_VALIDATOR
        .get_or_init(|| {
            let schema: Value =
                serde_json::from_str(SETTINGS_SCHEMA).map_err(|error| error.to_string())?;
            let validator =
                jsonschema::validator_for(&schema).map_err(|error| error.to_string())?;
            let hook_events = schema
                .get("properties")
                .and_then(|properties| properties.get("hooks"))
                .and_then(|hooks| hooks.get("properties"))
                .and_then(Value::as_object)
                .map(|events| events.keys().cloned().collect())
                .unwrap_or_default();
            Ok(SettingsValidator {
                validator,
                hook_events,
            })
        })
        .as_ref()
        .map_err(|error| ConfigError::Schema(error.clone()))
}

/// One traversal decides both what the UI sees and, through
/// [`restore_masked`], what gets written back. The name and value-shape rules
/// mirror the Servers path. `depth` 0 holds the settings' own top-level keys,
/// where native names such as `includeCoAuthoredBy` and
/// `workflowKeywordTriggerEnabled` accidentally contain the "auth"/"key"
/// substrings, so depth 0 masks only settings named exactly in
/// [`CREDENTIAL_SETTINGS`]. Below the top level, a string is masked when its
/// key name looks secret at any nesting depth, or when it sits inside an
/// `env` or `headers` object and its value looks like a credential.
fn masked_value(name: &str, value: &Value, depth: usize, in_secret_container: bool) -> Value {
    match value {
        Value::String(text) => {
            let nested = depth > 0;
            if (nested && secret_name(name))
                || (in_secret_container && value_shape_secret(text))
                || (depth == 0 && CREDENTIAL_SETTINGS.contains(&name) && !text.is_empty())
            {
                Value::String(MASK.into())
            } else {
                value.clone()
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| masked_value(name, item, depth + 1, in_secret_container))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(field, child)| {
                    let container =
                        in_secret_container || SECRET_CONTAINERS.contains(&field.as_str());
                    (
                        field.clone(),
                        masked_value(field, child, depth + 1, container),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Swap masked placeholders in `edited` back to the stored values at the
/// same name path. The stored document comes from a fresh disk read at the
/// mutation boundary, never from the snapshot the UI holds. Placeholders
/// inside arrays are never matched: array elements carry no name identity, so
/// a positional match could silently attach one stored credential to a
/// different entry after a reorder. Such edits refuse instead.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::HarnessPaths;
    use serde_json::json;
    use std::fs;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        project: PathBuf,
        claude_json: PathBuf,
        harness: ClaudeCodeHarness,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join(".claude");
            let project = dir.path().join("project");
            let claude_json = dir.path().join(".claude.json");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(&project).unwrap();
            let harness = ClaudeCodeHarness::new(HarnessPaths {
                config_root: root.clone(),
                claude_json: claude_json.clone(),
            });
            Self {
                _dir: dir,
                root,
                project,
                claude_json,
                harness,
            }
        }

        fn user_settings(&self) -> PathBuf {
            self.root.join("settings.json")
        }

        fn project_layer(&self) -> PathBuf {
            self.project.join(".claude").join("settings.json")
        }

        fn local_layer(&self) -> PathBuf {
            self.project.join(".claude").join("settings.local.json")
        }

        fn write(&self, path: &Path, value: Value) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
    }

    #[test]
    fn snapshot_comes_only_from_settings_layers_and_state_never_enters_it() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.user_settings(),
            json!({"model":"user", "theme":"dark"}),
        );
        fixture.write(
            &fixture.project_layer(),
            json!({"model":"project", "effortLevel":"high"}),
        );
        fixture.write(&fixture.local_layer(), json!({"model":"local"}));
        fixture.write(
            &fixture.claude_json,
            json!({
                "history":["fixture prompt history"],
                "projects":{"p":{"lastPrompt":"fixture per-project state"}},
                "mcpServers":{"a":{"type":"http","url":"https://fixture.invalid","headers":{"Authorization":"Bearer fixture-state-secret"}}},
                "oauthAccount":{"email":"fixture-account"}
            }),
        );

        let snapshot = load_config(&fixture.harness, &fixture.project).unwrap();
        assert_eq!(snapshot.effective["model"].value, json!("local"));
        assert_eq!(snapshot.effective["model"].source, "local");
        assert_eq!(snapshot.effective["theme"].source, "user");
        assert_eq!(snapshot.effective["effortLevel"].source, "project");
        assert!(!snapshot.effective.contains_key("history"));
        assert!(!snapshot.effective.contains_key("projects"));
        assert!(!snapshot.effective.contains_key("mcpServers"));

        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("claude_json"), "{serialized}");
        assert!(!serialized.contains("history"), "{serialized}");
        assert!(!serialized.contains("fixture prompt history"));
        assert!(!serialized.contains("fixture per-project state"));
        assert!(!serialized.contains("fixture-state-secret"));
        assert!(!serialized.contains("fixture-account"));
    }

    #[test]
    fn layer_paths_are_the_resolved_files_not_a_hardcoded_label() {
        let fixture = Fixture::new();
        let snapshot = load_config(&fixture.harness, &fixture.project).unwrap();
        let user = snapshot.layers.iter().find(|l| l.name == "user").unwrap();
        assert_eq!(
            user.path,
            fixture.user_settings().to_string_lossy().into_owned()
        );
        let local = snapshot.layers.iter().find(|l| l.name == "local").unwrap();
        assert_eq!(
            local.path,
            fixture.local_layer().to_string_lossy().into_owned()
        );
    }

    #[test]
    fn credential_bearing_settings_are_masked_in_every_serialized_value() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.user_settings(),
            json!({
                "model":"sonnet",
                "apiKeyHelper":"/bin/generate_fixture_key.sh",
                "awsAuthRefresh":"aws sso login --profile fixture-profile",
                "env":{"ANTHROPIC_API_KEY":"sk-fixture-credential","SAFE_MODE":"ok"},
                "hooks":{"PostToolUse":[{"matcher":"*","hooks":[{"type":"http","url":"https://fixture.invalid","headers":{"Authorization":"Bearer fixture-token","X-Custom-Service":"bearer fixture-named-secret"}}]}]}
            }),
        );

        let snapshot = load_config(&fixture.harness, &fixture.project).unwrap();
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("sk-fixture-credential"));
        assert!(!serialized.contains("generate_fixture_key"));
        assert!(!serialized.contains("fixture-token"));
        assert!(!serialized.contains("fixture-named-secret"));
        assert!(!serialized.contains("fixture-profile"));
        assert!(serialized.contains(MASK));
        assert_eq!(snapshot.effective["env"].value["ANTHROPIC_API_KEY"], MASK);
        assert_eq!(snapshot.effective["env"].value["SAFE_MODE"], "ok");
        assert_eq!(snapshot.effective["apiKeyHelper"].value, MASK);
        assert_eq!(snapshot.effective["awsAuthRefresh"].value, MASK);
        let http_hook = &snapshot.effective["hooks"].value["PostToolUse"][0]["hooks"][0];
        assert_eq!(http_hook["headers"]["Authorization"], MASK);
        assert_eq!(http_hook["headers"]["X-Custom-Service"], MASK);
        assert_eq!(http_hook["url"], "https://fixture.invalid");
        let user = snapshot.layers.iter().find(|l| l.name == "user").unwrap();
        assert_eq!(user.values["env"]["ANTHROPIC_API_KEY"], MASK);
    }

    #[test]
    fn applying_a_masked_edit_preserves_the_secret_and_a_fresh_placeholder_is_rejected() {
        let fixture = Fixture::new();
        let original = json!({
            "apiKeyHelper":"/bin/generate_fixture_key.sh",
            "env":{"ANTHROPIC_API_KEY":"sk-fixture-credential","SAFE_MODE":"ok"}
        });
        fixture.write(&fixture.user_settings(), original.clone());

        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "env",
            json!({"ANTHROPIC_API_KEY":MASK,"SAFE_MODE":"new"}),
        )
        .unwrap();
        let landed = atomic::read_json(&fixture.user_settings()).unwrap();
        assert_eq!(landed["env"]["ANTHROPIC_API_KEY"], "sk-fixture-credential");
        assert_eq!(landed["env"]["SAFE_MODE"], "new");

        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "apiKeyHelper",
            json!(MASK),
        )
        .unwrap();
        let landed = atomic::read_json(&fixture.user_settings()).unwrap();
        assert_eq!(landed["apiKeyHelper"], original["apiKeyHelper"]);

        let error = apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::Project,
            "env",
            json!({"NEW_TOKEN": MASK}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("masked placeholder"), "{error}");
        assert!(!fixture.project_layer().exists());
    }

    #[test]
    fn a_valid_edit_is_not_blocked_by_an_unrelated_schema_failure() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.user_settings(),
            json!({"env":{"legacy_name":"kept"},"theme":"dark"}),
        );

        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "model",
            json!("opus"),
        )
        .unwrap();

        assert_eq!(
            atomic::read_json(&fixture.user_settings()).unwrap(),
            json!({"env":{"legacy_name":"kept"},"theme":"dark","model":"opus"})
        );

        let error = apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "env",
            json!({"another_bad_name":"x"}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("/env"), "{error}");
    }

    #[test]
    fn unknown_and_native_keys_survive_an_apply() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.user_settings(),
            json!({"$schema":"https://json.schemastore.org/claude-code-settings.json","futureSetting":{"retained":true},"model":"sonnet"}),
        );

        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "model",
            json!("opus"),
        )
        .unwrap();

        assert_eq!(
            atomic::read_json(&fixture.user_settings()).unwrap(),
            json!({
                "$schema":"https://json.schemastore.org/claude-code-settings.json",
                "futureSetting":{"retained":true},
                "model":"opus"
            })
        );
    }

    #[test]
    fn native_settings_with_secret_like_names_survive_and_nested_credentials_do_not() {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.user_settings(),
            json!({
                "includeCoAuthoredBy":false,
                "workflowKeywordTriggerEnabled":true,
                "futurePlugin":{"serviceToken":"fixture-nested-secret","mode":"ok"}
            }),
        );

        let snapshot = load_config(&fixture.harness, &fixture.project).unwrap();
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            snapshot.effective["includeCoAuthoredBy"].value,
            json!(false)
        );
        assert_eq!(
            snapshot.effective["workflowKeywordTriggerEnabled"].value,
            json!(true)
        );
        assert!(
            !serialized.contains("fixture-nested-secret"),
            "{serialized}"
        );
        assert_eq!(
            snapshot.effective["futurePlugin"].value["serviceToken"],
            MASK
        );
        assert_eq!(snapshot.effective["futurePlugin"].value["mode"], "ok");
    }

    #[test]
    fn hooks_validation_tolerates_events_the_bundled_schema_does_not_know() {
        let fixture = Fixture::new();
        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "hooks",
            json!({"FutureEvent":[{"hooks":[{"type":"command","command":"echo"}]}]}),
        )
        .unwrap();
        assert_eq!(
            atomic::read_json(&fixture.user_settings()).unwrap()["hooks"]["FutureEvent"][0]
                ["hooks"][0]["command"],
            "echo"
        );
    }

    #[test]
    fn invalid_apply_leaves_the_settings_file_unchanged() {
        let fixture = Fixture::new();
        let path = fixture.user_settings();
        let original = json!({"model":"sonnet", "theme":"dark"});
        fixture.write(&path, original.clone());

        let error = apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "model",
            json!(17),
        )
        .unwrap_err();

        assert!(error.to_string().contains("/model"), "{error}");
        assert_eq!(atomic::read_json(&path).unwrap(), original);
    }

    #[test]
    fn a_stale_same_key_edit_refuses_without_writing_or_backing_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({"model":"rotated"})).unwrap(),
        )
        .unwrap();
        let backup = dir.path().join("backup.json");

        let receipt = atomic::commit(
            &path,
            vec![atomic::Patch::ReplacePath {
                path: atomic::p!("model"),
                matches: json!("sonnet"),
                value: json!("opus"),
            }],
            Some(&backup),
        );
        assert!(matches!(receipt, Err(atomic::CommitError::Stale { .. })));
        assert_eq!(atomic::read_json(&path).unwrap()["model"], "rotated");
        assert!(!backup.exists());
    }

    #[test]
    fn a_fresh_same_key_edit_lands_and_a_disjoint_key_composes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({"model":"sonnet","env":{"KEY":"v"}})).unwrap(),
        )
        .unwrap();
        let backup = dir.path().join("backup.json");

        atomic::commit(
            &path,
            vec![
                atomic::Patch::SetPath {
                    path: atomic::p!("model"),
                    value: json!("haiku"),
                },
                atomic::Patch::ReplacePath {
                    path: atomic::p!("env"),
                    matches: json!({"KEY":"v"}),
                    value: json!({"KEY":"v2"}),
                },
            ],
            Some(&backup),
        )
        .unwrap();

        assert_eq!(
            atomic::read_json(&path).unwrap(),
            json!({"model":"haiku","env":{"KEY":"v2"}})
        );
        assert!(backup.exists());
    }

    #[test]
    fn replace_path_refuses_missing_and_null_mismatched_slots() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({"explicit":null,"kept":1})).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            atomic::commit(
                &path,
                vec![atomic::Patch::ReplacePath {
                    path: atomic::p!("absent"),
                    matches: Value::Null,
                    value: json!(1),
                }],
                None,
            ),
            Err(atomic::CommitError::Stale { .. })
        ));
        atomic::commit(
            &path,
            vec![atomic::Patch::ReplacePath {
                path: atomic::p!("explicit"),
                matches: Value::Null,
                value: json!("filled"),
            }],
            None,
        )
        .unwrap();
        assert!(matches!(
            atomic::commit(
                &path,
                vec![atomic::Patch::ReplacePath {
                    path: atomic::p!("explicit"),
                    matches: json!("not-null"),
                    value: json!(2),
                }],
                None,
            ),
            Err(atomic::CommitError::Stale { .. })
        ));
        assert!(matches!(
            atomic::commit(
                &path,
                vec![atomic::Patch::ReplacePath {
                    path: atomic::p!("kept"),
                    matches: json!({"more":true}),
                    value: json!(3),
                }],
                None,
            ),
            Err(atomic::CommitError::Stale { .. })
        ));
        assert_eq!(
            atomic::read_json(&path).unwrap(),
            json!({"explicit":"filled","kept":1})
        );
    }

    #[test]
    fn a_masked_array_edit_refuses_and_an_unmasked_array_edit_lands() {
        let fixture = Fixture::new();
        let path = fixture.user_settings();
        let stored = json!({"tokens":[
            {"name":"alpha","token":"fixture-secret-one"},
            {"name":"beta","token":"fixture-secret-two"}
        ]});
        fixture.write(&path, stored.clone());

        let snapshot = load_config(&fixture.harness, &fixture.project).unwrap();
        let user = snapshot.layers.iter().find(|l| l.name == "user").unwrap();
        let mut edited = user.values["tokens"].clone();
        edited.as_array_mut().unwrap().reverse();
        let error = apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "tokens",
            edited,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("not matched by position"),
            "{error}"
        );
        assert_eq!(atomic::read_json(&path).unwrap(), stored);

        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "tokens",
            json!([
                {"name":"beta","token":"fixture-secret-two"},
                {"name":"alpha","token":"new-real-value"}
            ]),
        )
        .unwrap();
        assert_eq!(
            atomic::read_json(&path).unwrap()["tokens"][0]["name"],
            "beta"
        );
    }

    #[test]
    fn apply_changes_only_the_selected_key_in_the_selected_layer() {
        let fixture = Fixture::new();
        let path = fixture.user_settings();
        fixture.write(
            &path,
            json!({"model":"sonnet", "theme":"dark", "hooks":{"Stop":[]}}),
        );

        apply_value(
            &fixture.harness,
            &fixture.project,
            SettingsLayer::User,
            "model",
            json!("opus"),
        )
        .unwrap();

        assert_eq!(
            atomic::read_json(&path).unwrap(),
            json!({"model":"opus", "theme":"dark", "hooks":{"Stop":[]}})
        );
        assert!(!fixture.project_layer().exists());
    }
}
