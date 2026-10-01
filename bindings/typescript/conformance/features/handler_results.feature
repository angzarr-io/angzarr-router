@binding
Feature: Handler results across the FFI

  Undo and fact handlers may return nothing: an undo that returns nothing
  records no events, and a fact handler that returns nothing records the fact
  unchanged. An aggregate supports Replay when its state schema is known —
  given to the Rebuilder or attached to generated wiring — and refuses it
  otherwise.

  Scenario: an undo handler that returns nothing records no events
    Given a binding inventory aggregate whose AdjustStock undo returns nothing
    When a Compensate for AdjustStock is dispatched through the binding router
    Then the binding dispatch records no events
    And the undo handler saw the "inventory" cover

  Scenario: a fact handler that returns nothing records the fact unchanged
    Given a binding ledger aggregate whose Increased fact handler returns nothing
    When an Increased fact is dispatched through the binding router
    Then the binding router records one Increased fact

  Scenario: generated wiring with an attached state schema supports Replay
    Given a generated counter aggregate with its state schema attached
    When the binding router replays a snapshot of 4 then 3 Increased events
    Then the binding replay yields a counter of 7

  Scenario: an aggregate with no known state schema refuses Replay
    Given a generated counter aggregate
    When the binding router replays a snapshot of 4 then 3 Increased events
    Then the binding replay fails with NO_HANDLER_REGISTERED
