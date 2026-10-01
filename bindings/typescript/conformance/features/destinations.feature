@binding
Feature: Declared-output destinations

  A saga or process-manager handler sees the component's declared output
  domains, and nothing else, as its destinations. A process manager declares
  them through its constructor; one constructed without them has none.

  Scenario: a saga sees its declared target domains
    Given a binding saga targeting "inventory" and "billing"
    When an order event is dispatched to the binding saga
    Then the handler's destinations are "inventory" and "billing"
    And the handler's destinations do not include "ledger"

  Scenario: a process manager sees the target domains it was constructed with
    Given a binding process-manager targeting "inventory"
    When a counter trigger is dispatched to the binding process-manager
    Then the handler's destinations are "inventory"

  Scenario: a process manager constructed without targets has no destinations
    Given a binding process-manager with no targets
    When a counter trigger is dispatched to the binding process-manager
    Then the handler has no destinations
