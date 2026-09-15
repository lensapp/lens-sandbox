Feature: Live management updates
  Changes made by another client appear without a manual refresh.

  Scenario Outline: A completed management change wakes dashboard subscribers
    When another client receives "<response>" for a management action
    Then dashboard subscribers receive 1 management change notifications

    Examples:
      | response              |
      | ConnectorInstalled    |
      | ConnectorUninstalled  |
      | ConnectorConnected    |
      | ConnectorDisconnected |
      | ConnectorGranted      |
      | ConnectorForgotten    |
      | RegistryLoginStored   |
      | RegistryLoggedOut     |

  Scenario Outline: Reading data or refusing an action does not cause a refresh loop
    When another client receives "<response>" for a management action
    Then dashboard subscribers receive 0 management change notifications

    Examples:
      | response         |
      | ConnectorList    |
      | RegistryLogins   |
      | ConnectorUnknown |
      | Error            |
