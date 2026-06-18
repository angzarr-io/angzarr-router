Feature: Order process-manager dispatch

  The OrderProcessManager proves the stateful trigger-side dispatch the shared
  router must implement identically in every language: only the newest trigger
  page fires (history never re-triggers), a trigger from an unwatched domain or
  of an undeclared type does nothing, a missing or empty trigger is refused
  with a coded error, the PM's own state is rebuilt before the handler, and a
  rejection notification routes to the registered compensator with its
  escalation.

  Scenario: the newest trigger event reacts
    Given an order process-manager
    When an Increased trigger in domain "counter" is dispatched with destination inventory sequence 4
    Then the process-manager emits one command to "inventory"
    And the command carries destination sequence 4

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

  Scenario: a rejection routes to the compensator
    Given an order process-manager
    When a rejection of Reserve is dispatched
    Then the process-manager emits one process event
    And the process-manager escalates
