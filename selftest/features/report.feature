# Writes selftest/out like the other features, so they run one after the other.
@serial(selftest)
Feature: bddkit report turns an event stream into a page that keeps the masking
  # The stream comes from the masking fixture, whose scenarios fail on purpose.
  # The report is read back as text: nothing here opens a browser. It shows what
  # the stream holds and no more: a literal written in a step's own text is in
  # the stream before anything could mask it (events-design 7.11), so the two
  # secrets checked are the ones the stream never carries in the clear.

  Scenario: the HTML report of a masked run holds the mask and no secret
    When I run the masking fixture "bddkit" with options ""
    And I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/out/events.ndjson"
    Then the command exit code is 0
    When I run the command "cat selftest/out/events.html"
    Then the command output contains "***"
    And the command output contains "Content-Security-Policy"
    And the command output does not contain "literal-key-value-3"
    And the command output does not contain "config-secret-value-6"

  Scenario: a file that is not a stream is refused and nothing is written
    When I run the command "rm -rf selftest/out && mkdir -p selftest/out && echo nope > selftest/out/bad.ndjson; ${BDDKIT_BIN:-target/debug/bddkit} report selftest/out/bad.ndjson"
    Then the command exit code is 2
    And the command error output contains "nothing written"
    When I run the command "ls selftest/out"
    Then the command output does not contain "bad.html"

  Scenario: --embed-artifacts carries a linked file into the page and marks one it cannot find
    When I run the command "rm -rf selftest/out/embed && mkdir -p selftest/out/embed/art/1 && printf hello > selftest/out/embed/art/1/note.txt"
    And I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/fixtures/embed/run.ndjson --artifacts selftest/out/embed/art --embed-artifacts -o selftest/out/embed/r.html"
    Then the command exit code is 0
    And the command output contains "1 files embedded"
    And the command output contains "1 left as links"
    When I run the command "cat selftest/out/embed/r.html"
    # base64 of "hello": the file travels inside the page, not beside it
    Then the command output contains "aGVsbG8="
    And the command output contains "not found"

  Scenario: the embedding options are refused without the flag, and a quality without lossy
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/fixtures/embed/run.ndjson --embed-limit 1MB -o selftest/out/embed/no.html"
    Then the command exit code is 2
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/fixtures/embed/run.ndjson --embed-artifacts --embed-quality 70 -o selftest/out/embed/no.html"
    Then the command exit code is 2
    And the command error output contains "nothing written"
    When I run the command "ls selftest/out/embed"
    Then the command output does not contain "no.html"

  Scenario: the Markdown report holds the failures, their line ranges and no secret
    When I run the masking fixture "bddkit" with options ""
    And I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/out/events.ndjson -o selftest/out/events.md"
    Then the command exit code is 0
    When I run the command "cat selftest/out/events.md"
    Then the command output contains "bddkit_report: 1"
    And the command output contains "<!-- bddkit:begin failure"
    And the command output contains "***"
    And the command output does not contain "literal-key-value-3"
    And the command output does not contain "config-secret-value-6"

  Scenario: --detail needs a Markdown output
    When I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/fixtures/embed/run.ndjson --detail all -o selftest/out/embed/no.html"
    Then the command exit code is 2
    And the command error output contains "nothing written"

  Scenario: an index range of the Markdown report cuts out exactly its scenario
    When I run the masking fixture "bddkit" with options ""
    And I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/out/events.ndjson -o selftest/out/events.md"
    # the first index row is a failed scenario; its lines column is from-to
    When I run the command "r=selftest/out/events.md; range=$(grep -m1 '| failed |' $r | awk -F' [|] ' '{print $3}' | tr - ,); sed -n ${range}p $r | sed -n '1p;$p'"
    Then the command output contains "<!-- bddkit:begin scenario id="
    And the command output contains "<!-- bddkit:end scenario id="

  Scenario: every begin marker of the Markdown report has its end, and nothing else looks like one
    When I run the masking fixture "bddkit" with options ""
    And I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/out/events.ndjson -o selftest/out/events.md --detail all"
    When I run the command "r=selftest/out/events.md; b=$(grep -c '^<!-- bddkit:begin' $r); e=$(grep -c '^<!-- bddkit:end' $r); t=$(grep -c '^<!-- bddkit:' $r); test $b -gt 0 && test $b = $e && test $t = $((b + e)) && echo paired"
    Then the command output contains "paired"
    When I run the command "grep -c 'detail: all' selftest/out/events.md"
    Then the command output contains "1"

  Scenario: the Markdown report links an artifact relative to itself and names the failure
    When I run the command "rm -rf selftest/out/embed && mkdir -p selftest/out/embed/art/1"
    And I run the command "${BDDKIT_BIN:-target/debug/bddkit} report selftest/fixtures/embed/run.ndjson --artifacts selftest/out/embed/art -o selftest/out/embed/r.md"
    Then the command exit code is 0
    When I run the command "cat selftest/out/embed/r.md"
    Then the command output contains "(art/1/note.txt)"
    And the command output contains "### [failed] s"
    And the command output contains "begin scenario id="
    And the command output contains "a.feature:1"
    And the command output contains "boom"
    And the command output contains "complete: true"
