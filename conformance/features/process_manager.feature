Feature: Order process-manager dispatch

  The OrderProcessManager proves the stateful trigger-side dispatch the shared
  router must implement identically in every language: only the newest trigger
  page fires (history never re-triggers) and its commands pass through
  unstamped, a trigger from an unwatched domain or of an undeclared type does
  nothing, a missing or empty trigger is refused with a coded error, and the
  PM's own state is rebuilt before the handler. The coordinator stamps the
  commands' identity and addresses the process events.

  Scenario: the newest trigger event reacts with a command
    Given an order process-manager
    When an Increased trigger in domain "counter" at sequence 4 is dispatched
    Then the process-manager emits one command to "inventory"
    And the command carries no router-stamped provenance

  Scenario: history does not re-trigger
    Given an order process-manager
    When a trigger whose newest page is an undeclared event is dispatched
    Then the process-manager emits no commands

  Scenario: a trigger from an unwatched domain does nothing
    Given an order process-manager
    When an Increased trigger in domain "billing" is dispatched
    Then the process-manager emits no commands

  Scenario: the process-manager state is rebuilt before the handler
    Given an order process-manager
    When an Increased trigger is dispatched over a prior state of 3 events
    Then the process-manager rebuilt 3 prior state events

  Scenario: a missing trigger is refused
    Given an order process-manager
    When a request with no trigger is dispatched
    Then the dispatch fails with MISSING_PM_TRIGGER

  Scenario: an empty trigger is refused
    Given an order process-manager
    When a trigger with no pages is dispatched
    Then the dispatch fails with EMPTY_PM_TRIGGER

  Scenario: an unaddressed process event is left for the coordinator to address
    Given an order process-manager
    When an Increased trigger in domain "counter" is dispatched
    Then the process event carries no cover
