@f
Feature: Tags on Examples blocks
  @s
  Scenario Outline: Row <n>
    Given set variable "a" to "<n>"

    @slow
    Examples:
      | n |
      | 1 |
      | 2 |

    Examples:
      | n |
      | 3 |
