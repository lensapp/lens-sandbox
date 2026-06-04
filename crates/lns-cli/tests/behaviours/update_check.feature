Feature: lns surfaces the service's update-and-security check
  lns-service performs an update-and-security check about once an hour and
  marks the latest available version. The lns CLI only reads that marked
  result — it never contacts the network, so commands are never slowed —
  and tells the user to run `lns update` when a newer or insecure version
  is available.

  Scenario: A marked newer version tells the user to run lns update
    Given the service has marked a newer version available
    When the user runs an lns command
    Then lns tells the user to run "lns update"
    And lns does not contact the network or download anything

  Scenario: A marked security update is surfaced
    Given the service has marked a security update for the running version
    When the user runs an lns command
    Then the user is told to run "lns update" because a security update is available

  Scenario: No nagging when the running version is current
    Given the service has marked the running version as latest
    When the user runs an lns command
    Then lns prints no update message

  Scenario: Quiet when the service has not marked anything yet
    Given the service has not marked any version yet
    When the user runs an lns command
    Then lns prints no update message

  Scenario: A user can inspect exactly what the service sends
    When the user runs the update check in dry-run mode
    Then lns prints the payload — install ID, version, and OS/arch — and contacts nothing

  @todo
  Scenario: The install script records an install count without identifying the user
    When the install script fetches lns from get.lns.run
    Then the request is counted server-side with no user-identifying information
