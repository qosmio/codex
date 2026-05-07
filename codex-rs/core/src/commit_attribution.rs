const DEFAULT_ATTRIBUTION_VALUE: &str = "Codex <noreply@openai.com>";

const PUBLIC_CONTRIBUTION_INSTRUCTION: &str = r#"## PUBLIC CONTRIBUTION MODE

You are preparing changes for a public or open-source repository.

Commit messages, branch names, PR titles, and PR bodies must describe only the
code change. Do not include AI attribution, internal tool names, private project
names, model names, generated-by text, Co-authored-by trailers, or references to
agentic workflows.

Write publication text as a human developer would. If this conflicts with
general attribution guidance, this public contribution mode controls publication
surfaces."#;

pub(crate) struct CommitPublicationConfig<'a> {
    pub(crate) commit_attribution: Option<&'a str>,
    pub(crate) public_contribution_mode: bool,
}

pub(crate) fn commit_publication_instructions(config: CommitPublicationConfig<'_>) -> Vec<String> {
    if config.public_contribution_mode {
        vec![PUBLIC_CONTRIBUTION_INSTRUCTION.to_string()]
    } else {
        commit_message_trailer_instruction(config.commit_attribution)
            .into_iter()
            .collect()
    }
}

fn build_commit_message_trailer(config_attribution: Option<&str>) -> Option<String> {
    let value = resolve_attribution_value(config_attribution)?;
    Some(format!("Co-authored-by: {value}"))
}

pub(crate) fn commit_message_trailer_instruction(
    config_attribution: Option<&str>,
) -> Option<String> {
    let trailer = build_commit_message_trailer(config_attribution)?;
    Some(format!(
        "When you write or edit a git commit message, ensure the message ends with this trailer exactly once:\n{trailer}\n\nRules:\n- Keep existing trailers and append this trailer at the end if missing.\n- Do not duplicate this trailer if it already exists.\n- Keep one blank line between the commit body and trailer block."
    ))
}

fn resolve_attribution_value(config_attribution: Option<&str>) -> Option<String> {
    match config_attribution {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => Some(DEFAULT_ATTRIBUTION_VALUE.to_string()),
    }
}

#[cfg(test)]
#[path = "commit_attribution_tests.rs"]
mod tests;
