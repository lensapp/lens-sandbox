@todo
Feature: what the disclosure says about a connector that carries code
  A declarative connector can be read before it is trusted: the disclosure shows
  the destinations it opens, the variables it sets, and the files it writes, and
  that is the whole of what it will do. A component cannot be read, so the
  disclosure shows bounds instead of behaviour — the hosts it may contact, and the
  digest the connector is installed at, which covers the component's bytes.

  A method that declares host execution gets the weaker of the two guarantees, and
  the stronger sentence is then forbidden rather than merely unused: lns bounds
  where the component runs and how long it has, but nothing about what a program it
  starts may reach. A disclosure that overstates what lns enforces is worse than
  none, so both sentences are carried verbatim and only one of them may appear.

  Scenario: the disclosure states bounds rather than behaviour
    Given a connector whose method "sign-in" is a code method
    And the method declares the hosts "auth.some-provider.example" and "api.some-provider.example"
    And the method declares no host execution
    When the user runs connector command "grant some-provider --run 1a2b3c4d --method sign-in"
    Then the disclosure names both hosts it may contact
    And the disclosure names the digest the connector is installed at
    And the disclosure says "lns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has."

  Scenario: a method declaring host execution cannot claim the stronger sentence
    Given a connector whose method "sign-in" is a code method
    And the method declares host execution
    When the user runs connector command "grant some-provider --run 1a2b3c4d --method sign-in"
    Then the disclosure says "lns cannot show what this code does, and it runs programs on your machine with your own access. lns cannot bound what those reach."
    And the disclosure does not say "lns cannot show what this code does. It can only bound where it runs, what it reaches, and how long it has."

  Scenario: a component that reaches nothing says so rather than saying nothing
    Given a connector whose method "sign-in" is a code method
    And the method declares no hosts
    When the user runs connector command "grant some-provider --run 1a2b3c4d --method sign-in"
    Then the disclosure says "it may contact no hosts."
