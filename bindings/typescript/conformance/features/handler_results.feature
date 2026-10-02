@binding
Feature: Handler results across the FFI

  An undo handler may return nothing, which records no events. A fact handler
  returns a FactRecord: FactRecord.asReceived records the fact as received
  with no flags, and each flag is recorded after its fact with no header. An aggregate supports Replay when its state schema is known —
  given to the Rebuilder, as generated wiring does — and refuses it
  otherwise.

  Scenario: an undo handler that returns nothing records no events
    Given a binding inventory aggregate whose AdjustStock undo returns nothing
    When a Compensate for AdjustStock is dispatched through the binding router
    Then the binding dispatch records no events
    And the undo handler saw the "inventory" cover

  Scenario: a fact recorded as received is recorded unchanged
    Given a binding ledger aggregate whose Increased fact handler records it as received
    When an Increased fact is dispatched through the binding router
    Then the binding router records one Increased fact

  Scenario: a fact record's flags follow its fact, with no header
    Given a binding ledger aggregate whose Increased fact handler flags it with two Increased events
    When an Increased fact is dispatched through the binding router
    Then the binding router records the Increased fact then 2 headerless Increased flags

  Scenario: a fact handler that returns no FactRecord fails as unhandled
    Given a binding ledger aggregate whose Increased fact handler returns nothing
    When an Increased fact is dispatched through the binding router
    Then the binding fact dispatch fails with UNHANDLED_HANDLER_ERROR

  Scenario: generated wiring supports Replay
    Given a generated counter aggregate
    When the binding router replays a snapshot of 4 then 3 Increased events
    Then the binding replay yields a counter of 7

  Scenario: an aggregate with no known state schema refuses Replay
    Given a binding counter aggregate with no state schema
    When the binding router replays a snapshot of 4 then 3 Increased events
    Then the binding replay fails with NO_HANDLER_REGISTERED
