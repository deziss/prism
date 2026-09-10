# Filter rules

Drop any of these in `~/.config/prism/filters/` (or point `PRISM_FILTER_RULES_DIR`
somewhere else) and `prism cmd <tool>` starts filtering that tool — no rebuild.

`prism config --show` lists the rules that loaded and the files that failed. An unknown
`shape`, an unknown `cap`, or a misspelled key is a reported error, not a silent no-op,
and one bad file does not disable the others.

A rule applies only where prism's built-in dispatch would have fallen through to
`generic`. To take precedence over a built-in Rust filter, add `override: true`.

## What YAML can and cannot do

`shape` selects an existing Rust primitive; the YAML never expresses the parsing. That is
the point, not a limitation to route around: a regex keep/drop language would be more
expressive and strictly worse, because regex can decide to drop a line but cannot *count*
what it dropped — and every `[+N more …]` marker in prism is arithmetic over parsed
structure. A rule handles a tool that prints a shape prism already understands. A
genuinely new shape needs a Rust filter, and should.
