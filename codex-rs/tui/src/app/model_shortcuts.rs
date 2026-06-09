use super::*;

enum ModelShortcutAction {
    Previous,
    Next,
    Select(String),
}

impl App {
    pub(super) fn handle_model_shortcuts(&mut self, key_event: KeyEvent) -> bool {
        let Some(action) = self.model_shortcut_action(key_event) else {
            return false;
        };

        if !self.chat_widget.is_session_configured() {
            self.chat_widget.add_info_message(
                "Model selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return true;
        }

        let models = self
            .chat_widget
            .model_catalog()
            .try_list_models()
            .unwrap_or_default();
        if models.is_empty() {
            self.chat_widget.add_info_message(
                "Models are being updated; please try /model again in a moment.".to_string(),
                /*hint*/ None,
            );
            return true;
        }

        match action {
            ModelShortcutAction::Previous => {
                self.select_adjacent_model(&models, /*forward*/ false);
            }
            ModelShortcutAction::Next => {
                self.select_adjacent_model(&models, /*forward*/ true);
            }
            ModelShortcutAction::Select(model_id) => {
                self.select_model_by_id(&models, &model_id);
            }
        }
        true
    }

    fn model_shortcut_action(&self, key_event: KeyEvent) -> Option<ModelShortcutAction> {
        if self.keymap.model.previous.is_pressed(key_event) {
            return Some(ModelShortcutAction::Previous);
        }
        if self.keymap.model.next.is_pressed(key_event) {
            return Some(ModelShortcutAction::Next);
        }

        self.keymap
            .model
            .bindings
            .iter()
            .find(|(_, bindings)| bindings.iter().any(|binding| binding.is_press(key_event)))
            .map(|(model_id, _)| ModelShortcutAction::Select(model_id.clone()))
    }

    fn select_adjacent_model(&mut self, models: &[ModelPreset], forward: bool) {
        let current_model = self.chat_widget.current_model();
        let Some(index) = models
            .iter()
            .position(|preset| preset.model == current_model)
        else {
            let target = if forward {
                &models[0]
            } else {
                &models[models.len() - 1]
            };
            self.apply_model_shortcut_selection(
                target.model.clone(),
                Some(target.default_reasoning_effort.clone()),
            );
            return;
        };
        let target_index = if forward {
            (index + 1) % models.len()
        } else {
            (index + models.len() - 1) % models.len()
        };
        let target = &models[target_index];
        self.apply_model_shortcut_selection(
            target.model.clone(),
            Some(target.default_reasoning_effort.clone()),
        );
    }

    fn select_model_by_id(&mut self, models: &[ModelPreset], model_id: &str) {
        let Some(target) = models.iter().find(|preset| preset.id == model_id) else {
            self.chat_widget.add_info_message(
                format!("Model `{model_id}` is no longer available."),
                /*hint*/ None,
            );
            return;
        };

        self.apply_model_shortcut_selection(
            target.model.clone(),
            Some(target.default_reasoning_effort.clone()),
        );
    }

    fn apply_model_shortcut_selection(
        &mut self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        self.app_event_tx.send(AppEvent::UpdateModel(model.clone()));
        self.app_event_tx
            .send(AppEvent::UpdateReasoningEffort(effort.clone()));
        self.app_event_tx
            .send(AppEvent::PersistModelSelection { model, effort });
    }
}
