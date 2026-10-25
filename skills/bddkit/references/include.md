# `I include`

Runs a scenario from another feature file as one step.

```gherkin
# shared/team.feature — outside `paths`
Feature: team setup

  @exports(team_id)
  Scenario Outline: create a team
    Given the request body is:
      """
      {"title": "<title>", "tier": "<tier>"}
      """
    When I request "/api/v1/teams" using HTTP POST
    Then the response code is 201
    And extract "id" from JSON as "team_id"

    Examples:
      | title | tier |
      | -     | -    |
```

```gherkin
Given I include "../shared/team.feature" with prefix "from" with:
  | title              | tier       |
  | from-<<unique()>>  | enterprise |
And I include "../shared/team.feature" with prefix "to" with:
  | title              | tier       |
  | to-<<unique()>>    | enterprise |
When I request "/api/v1/transfers/<<from_team_id>>/<<to_team_id>>" using HTTP POST
```

- The path is relative to the file the step is in. Keep the helper outside `paths`, or it also runs alone.
- The helper sees none of the caller's variables. Only `@exports(a,b)` (`@exports(*)`: all) come back. Extract inside the helper, right after the request; never `extract` from the response the include left behind.
- HTTP state, the current API and connection, and plugin instances are shared.
- `with:` needs a `Scenario Outline` with an `Examples:` row. It has one data row and a column per placeholder; cells may use the caller's `<<vars>>`. Without `with:` the `Examples:` row runs as written, so make it fake.
- `with prefix "p"` returns `p_<name>`, before `with:`.
- `scenario "<name>"` picks one scenario from a file with several.
- A credential column has `secret` in its name (`password_secret`).
