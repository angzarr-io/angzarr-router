Feature: Order policy dispatch

  The OrderPolicy proves the translation-side dispatch mechanisms the shared
  router must implement identically in every language: a declared source
  event emits a command (passed through unstamped: the coordinator stamps
  its identity and provenance) and its handler sees the triggering event's
  sequence, an undeclared event emits nothing, and an empty source is
  refused with a coded error.

  Scenario: a declared event emits a command
    Given an order policy delivering to "inventory"
    When an Increased event at sequence 7 is dispatched
    Then the policy emits one command to "inventory"
    And the command carries no router-stamped provenance

  Scenario: an undeclared event emits nothing
    Given an order policy delivering to "inventory"
    When a Reserve event is dispatched
    Then the policy emits no commands

  Scenario: a policy handler sees the triggering event's sequence
    Given an order policy delivering to "inventory"
    When an Increased event at sequence 7 is dispatched
    Then the policy handler saw source sequence 7
