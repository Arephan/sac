# large-redacted

A shape-only redaction of one real, large setup, produced with
`node setup.mjs redact -i <instance> --out examples/large-redacted`.

Kept: how many items each layer has, and enum-like fields (account kind and access, rule
severity/scope/date/status, loop runner/enabled/schedule, MCP transport, hook event, policy check).
Dropped: every name, path, handle, id, description and reason. Use it to see what a mature setup
looks like at scale; use `examples/minimal` to see what the fields mean.

    node setup.mjs validate -i examples/large-redacted
