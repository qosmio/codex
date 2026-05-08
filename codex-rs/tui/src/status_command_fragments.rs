pub(crate) fn is_status_command_fragment(text: &str) -> bool {
    matches!(text, "status" | "tatus$" | "atus" | "tus" | "us" | "s")
}
