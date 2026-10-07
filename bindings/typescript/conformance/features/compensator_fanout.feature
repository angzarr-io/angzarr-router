@binding
Feature: Ordered compensator fan-out

  Two compensators registered for one rejected command on one aggregate both
  run across the FFI, in registration order, and their events merge in order.

  Scenario: two compensators run in registration order
    Given a binding counter aggregate with two Reserve compensators
    When a Reserve rejection is dispatched through the binding router
    Then the compensators ran first then second
    And the compensation merged 2 events, the first from the first compensator
