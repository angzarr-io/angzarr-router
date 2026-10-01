Feature: Order saga dispatch

  The OrderSaga proves the translation-side dispatch mechanisms the shared
  router must implement identically in every language: a declared source
  event emits a deferred command (angzarr_deferred provenance from the
  triggering event, never an explicit sequence) and its handler sees the
  triggering event's sequence, an undeclared event emits
  nothing, a source missing or empty is refused with a coded error, and a
  rejection notification in a saga's source emits nothing (sagas receive no
  rejections).

  Scenario: a declared event emits a deferred command
    Given an order saga delivering to "inventory"
    When an Increased event at sequence 7 is dispatched
    Then the saga emits one command to "inventory"
    And the command is deferred from source sequence 7 at command index 0

  Scenario: an undeclared event emits nothing
    Given an order saga delivering to "inventory"
    When a Reserve event is dispatched
    Then the saga emits no commands

  Scenario: a source with no pages is refused
    Given an order saga delivering to "inventory"
    When a source with no pages is dispatched
    Then the dispatch fails with EMPTY_SAGA_SOURCE

  Scenario: a request with no source is refused
    Given an order saga delivering to "inventory"
    When a request with no source is dispatched
    Then the dispatch fails with MISSING_SAGA_SOURCE

  Scenario: a rejection in the source is not a saga's to compensate
    Given an order saga delivering to "inventory"
    When a rejection of Reserve is dispatched
    Then the saga emits no commands
    And the saga injects no events

  Scenario: a saga handler sees the triggering event's sequence
    Given an order saga delivering to "inventory"
    When an Increased event at sequence 7 is dispatched
    Then the saga handler saw source sequence 7
