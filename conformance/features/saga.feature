Feature: Order saga dispatch

  The OrderSaga proves the translation-side dispatch mechanisms the shared
  router must implement identically in every language: a declared source
  event emits a command stamped with the coordinator-supplied destination
  sequence, an undeclared event emits nothing, a source missing or empty is
  refused with a coded error, and a rejection notification routes to the
  registered compensator while an unwatched rejection is ignored.

  Scenario: a declared event emits a stamped command
    Given an order saga delivering to "inventory"
    When an Increased event is dispatched with destination inventory sequence 7
    Then the saga emits one command to "inventory"
    And the command carries destination sequence 7

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

  Scenario: a rejection routes to the compensator
    Given an order saga delivering to "inventory"
    When a rejection of Reserve is dispatched
    Then the saga injects one fact event

  Scenario: an unwatched rejection is ignored
    Given an order saga delivering to "inventory"
    When a rejection of Unwatched is dispatched
    Then the saga injects no events
