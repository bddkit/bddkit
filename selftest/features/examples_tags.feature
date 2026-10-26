# Writes selftest/out, so it runs one after the other with the files that do.
@serial(selftest)
Feature: A row carries the tags of its Examples block
  # The fixture is one outline with two Examples blocks, @slow on the first only.

  Scenario: --tag picks the rows of the tagged block and the stream lists its tag last
    When I run the examples tags fixture with options "--tag slow"
    Then the command output contains "Row <n> [1]=@s,@f,@slow|Row <n> [2]=@s,@f,@slow|"
    And the command output does not contain "[3]"

  Scenario: the other block's row does not carry the tag
    When I run the examples tags fixture with options ""
    Then the command output contains "Row <n> [3]=@s,@f|"
