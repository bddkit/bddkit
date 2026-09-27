@demo @include
Feature: `I include` — reusing scenarios from another file

  Scenario: includes a named scenario and reads back its export
    Given I include "include_setup.feature" scenario "a user signs up"
    Then variable "userId" should be equal to "abc123"
    # "internalOnly" was set inside include_setup.feature but never
    # exported — it does not exist in this scenario's variables at all.
    # Illustrated by this comment only, not proven by an assertion: no step
    # in this codebase can assert a variable's absence without failing the
    # scenario, so referencing "internalOnly" here would break the demo.

  Scenario: includes a Scenario Outline with a caller-supplied row
    Given set variable "myValue" to "from-caller"
    When I include "include_setup.feature" scenario "records what it was given" with:
      | value |
      | <<myValue>> |
    Then variable "seen" should be equal to "from-caller"

  Scenario: includes the same flow twice with a call-site prefix
    Given I include "include_setup.feature" scenario "a user signs up" with prefix "buyer"
    And I include "include_setup.feature" scenario "a user signs up" with prefix "seller"
    Then variable "buyer_userId" should be equal to "abc123"
    And variable "seller_userId" should be equal to "abc123"
