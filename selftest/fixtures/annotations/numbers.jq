# The step number of every step event; a disabled step never ran and has none.
select(.type | test("^step_(started|skipped|disabled)$"))
| "\(.type)=\(.step // "-")"
