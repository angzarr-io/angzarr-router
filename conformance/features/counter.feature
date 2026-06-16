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

  Scenario: a command carrying no command book is refused
    When a command with no command book is dispatched
    Then the command fails with MISSING_COMMAND_BOOK

  Scenario: a command book with no pages is refused
    When a command with an empty command book is dispatched
    Then the command fails with MISSING_COMMAND_PAGE

  Scenario: a command page with no payload is refused
    When a command whose page carries no payload is dispatched
    Then the command fails with MISSING_COMMAND_PAYLOAD

  Scenario: a corrupt persisted event fails the command on rebuild
    Given a counter whose history holds a corrupt event
    When the operator increases the counter by 1
    Then the command fails with PERSISTED_EVENT_CORRUPT

  Scenario: emitted events inherit the command's parent linkage
    Given a new counter
    When the operator increases the counter by 1 on behalf of a parent
    Then the recorded events carry the parent linkage

  Scenario: a rejected command runs its compensators in registration order
    Given a new counter
    When a Reserve command is rejected
    Then the compensations run first then second

  Scenario: a rejection with no registered compensator is left to the framework
    Given a new counter
    When an unregistered command is rejected
    Then no compensation is recorded

  Scenario: a fresh counter supplies no prior-history evidence to the handler
    Given a new counter
    When the operator increases the counter by 1
    Then the handler saw no prior history, at next sequence 0

  Scenario: prior events are reported to the handler as historical evidence
    Given a counter that has already recorded 2 increases
    When the operator increases the counter by 1
    Then the handler saw prior history, at next sequence 2

  Scenario: a snapshot seeds state and its covered page is not refolded
    Given a counter restored from a snapshot of 10 with one newer event
    When the operator increases the counter by 1
    Then the handler saw a counter of 11, at next sequence 12
