@binding
Feature: Coded errors carrying gRPC OK

  A handler failure is never a success: a CodedError whose gRPC code is OK (0)
  crosses the boundary as INVALID_ARGUMENT, keeping the handler's reason code.

  Scenario: a coded error with gRPC OK is a rejection
    Given a binding counter aggregate whose handler throws NOT_ALLOWED with gRPC code 0
    When a command is dispatched through the binding router
    Then the binding dispatch fails with NOT_ALLOWED as INVALID_ARGUMENT
