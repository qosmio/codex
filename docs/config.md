# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Public contribution mode

Enable `public_contribution_mode` when preparing changes for a public or
open-source repository and you want generated publication text to avoid AI
attribution and private implementation details:

```toml
public_contribution_mode = true
```

When enabled, Codex instructs the model to keep commit messages, branch names,
PR titles, and PR bodies focused only on the code change. It also suppresses
Codex's automatic `Co-authored-by:` trailer instruction even when
`codex_git_commit` is enabled.
