Feature: Counter aggregate dispatch

  The CounterAggregate proves the dispatch mechanisms the shared router must
  implement identically in every language: prior events fold into state,
  commands emit events with historical context, business rules reject, and
  unclassified or misrouted commands surface as coded errors.

  Scenario Outline: increasing a new counter records that many events
    Given a new counter
    When the operator increases the counter by <amount>
    Then <amount> increases are recorded, starting at sequence 0

    Examples:
      | amount |
      | 1      |
      | 2      |
      | 5      |

  Scenario: a prior increase folds in before the next command
    Given a counter that has already recorded 1 increase
    When the operator increases the counter by 2
    Then 2 increases are recorded, continuing from sequence 1

  Scenario: increasing by zero is rejected and records nothing
    Given a new counter
    When the operator increases the counter by 0
    Then the command is rejected as VALUE_NOT_POSITIVE
    And no events are recorded

  Scenario: an unclassified failure surfaces as a coded error
    Given a new counter
    When the operator triggers a hard failure
    Then the command fails with UNHANDLED_HANDLER_ERROR

  Scenario: an unknown command is reported before any rebuild
    Given a new counter
    When an unhandled command is dispatched
    Then the command fails with NO_HANDLER_REGISTERED
