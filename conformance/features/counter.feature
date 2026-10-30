Feature: Counter aggregate dispatch

  The CounterAggregate proves the dispatch mechanisms the shared router must
  implement identically in every language: prior events fold into state,
  commands emit events with historical context (passed through as the
  handler emitted them: the coordinator stamps sequences and meta), business
  rules reject, and unclassified or misrouted commands surface as coded errors.

  Scenario Outline: increasing a new counter records that many events
    Given a new counter
    When the operator increases the counter by <amount>
    Then <amount> increases are recorded
    And the recorded events carry no router-stamped sequence

    Examples:
      | amount |
      | 1      |
      | 2      |
      | 5      |

  Scenario: a prior increase folds in before the next command
    Given a counter that has already recorded 1 increase
    When the operator increases the counter by 2
    Then 2 increases are recorded
    And the handler saw prior history, at next sequence 1

  Scenario: increasing by zero is rejected and records nothing
    Given a new counter
    When the operator increases the counter by 0
    Then the command is rejected as VALUE_NOT_POSITIVE
    And no events are recorded

  Scenario: an unclassified failure surfaces as a coded error
    Given a new counter
    When the operator triggers a hard failure
    Then the command fails with UNHANDLED_HANDLER_ERROR

  # command_handler.proto, CommandHandlerService.Handle step 2: a type URL
  # that reaches no registered handler is refused, naming the type URL
  # (decision 12.11; backlog VC-1), under its own key UNKNOWN_COMMAND_TYPE,
  # INTERNAL: the router's own malformed-input error is a bug (decision 14.37).
  Scenario: an unknown command is reported before any rebuild
    Given a new counter
    When an unhandled command is dispatched
    Then the command fails with UNKNOWN_COMMAND_TYPE
    And the refusal is INTERNAL naming the type URL "type.googleapis.com/test.counter.Reserve"

  Scenario: a command carrying no command book is refused
    When a command with no command book is dispatched
    Then the command fails with MISSING_COMMAND_BOOK

  Scenario: a command book with no pages is refused
    When a command with an empty command book is dispatched
    Then the command fails with MISSING_COMMAND_PAGE

  Scenario: a command page with no payload is refused
    When a command whose page carries no payload is dispatched
    Then the command fails with MISSING_COMMAND_PAYLOAD

  # A failing fold has its own key, FOLD_FAILED (decision 14.37).
  Scenario: a persisted event the fold cannot fold fails the command as a handler bug
    Given a counter whose history holds a corrupt event
    When the operator increases the counter by 1
    Then the command fails with FOLD_FAILED

  Scenario: a fresh counter supplies no prior-history evidence to the handler
    Given a new counter
    When the operator increases the counter by 1
    Then the handler saw no prior history, at next sequence 0

  Scenario: prior events are reported to the handler as historical evidence
    Given a counter that has already recorded 2 increases
    When the operator increases the counter by 1
    Then the handler saw prior history, at next sequence 2

  Scenario Outline: commands and events route by fully-qualified name under any type-URL prefix
    Given a counter that has already recorded 2 increases under the "<prefix>" type-URL prefix
    When the operator increases the counter by 1 under the "<prefix>" type-URL prefix
    Then the handler saw a counter of 2, at next sequence 2

    Examples:
      | prefix               |
      | type.googleapis.com/ |
      | /                    |
      | example.com/types/   |
