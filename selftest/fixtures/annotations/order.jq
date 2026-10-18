# One word per structural event, in stream order. Quotes become apostrophes so
# a step can name the line it expects without escaping.
select(.type | test("^(scenario_started|step_started|step_skipped|step_disabled|section)$"))
| "\(.type) \(.title // .text // .name)" | gsub("\""; "'")
