Feature: Order process-manager dispatch

  The OrderProcessManager proves the stateful trigger-side dispatch the shared
  router must implement identically in every language: only the newest trigger
  page fires (history never re-triggers) and its commands are deferred from
  that page, a trigger from an unwatched domain or
  of an undeclared type does nothing, a missing or empty trigger is refused
  with a coded error, and the PM's own state is rebuilt before the handler. A
  process event the handler leaves unaddressed belongs to the process-manager's
  own domain.

  Co-resident process-managers share one router: each rebuilds only its own
  state (never another PM's, even within one dispatch), and a process state
  reaches only the PM that owns it.

  Scenario: the newest trigger event reacts with a deferred command
    Given an order process-manager
    When an Increased trigger in domain "counter" at sequence 4 is dispatched
    Then the process-manager emits one command to "inventory"
    And the command is deferred from source sequence 4 at command index 0
    And the command leaves its source component to the coordinator

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

  Scenario: co-resident process-managers each rebuild only their own state
    Given co-resident order and audit process-managers
    When an Increased trigger is dispatched over a prior state of 3 events
    Then the order process-manager rebuilt 3 prior state events
    And the audit process-manager rebuilt 3 prior state events

  Scenario: a process state reaches only the process-manager that owns it
    Given co-resident order and audit process-managers
    When an Increased trigger is dispatched over a prior "audit-pm" state of 2 events
    Then the audit process-manager rebuilt 2 prior state events
    And the order process-manager did not react

  Scenario: an unaddressed process event belongs to the process-manager's own domain
    Given an order process-manager
    When an Increased trigger in domain "counter" is dispatched
    Then the process event is addressed to "order-pm"
