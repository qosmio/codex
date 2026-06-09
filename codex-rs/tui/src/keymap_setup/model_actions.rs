use std::collections::BTreeSet;

use codex_protocol::openai_models::ModelPreset;

pub(crate) const MODEL_BINDINGS_CONTEXT: &str = "model.bindings";
pub(crate) const MODEL_CONTEXT_LABEL: &str = "Models";

#[derive(Clone, Debug)]
pub(crate) struct ModelActionMetadata {
    pub(crate) label: String,
    pub(crate) description: String,
}

pub(crate) fn is_model_binding_action(context: &str, action: &str) -> bool {
    context == MODEL_BINDINGS_CONTEXT && !matches!(action, "previous" | "next")
}

pub(crate) fn model_binding_config_path(action: &str) -> String {
    format!("tui.keymap.{MODEL_BINDINGS_CONTEXT}.{action}")
}

pub(crate) fn model_action_config_path(context: &str, action: &str) -> String {
    if is_model_binding_action(context, action) {
        model_binding_config_path(action)
    } else {
        format!("tui.keymap.{context}.{action}")
    }
}

pub(crate) fn model_action_ids(
    model_presets: &[ModelPreset],
    configured_ids: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = BTreeSet::new();
    for id in model_presets
        .iter()
        .map(|preset| preset.id.clone())
        .chain(configured_ids)
    {
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    ids
}

pub(crate) fn model_action_metadata(
    model_presets: &[ModelPreset],
    action: &str,
) -> ModelActionMetadata {
    let Some(preset) = model_presets.iter().find(|preset| preset.id == action) else {
        return ModelActionMetadata {
            label: action.to_string(),
            description: "Model preset is no longer in the current catalog.".to_string(),
        };
    };

    ModelActionMetadata {
        label: preset.display_name.clone(),
        description: if preset.description.is_empty() {
            let model = &preset.model;
            format!("Model slug: {model}")
        } else {
            preset.description.clone()
        },
    }
}
