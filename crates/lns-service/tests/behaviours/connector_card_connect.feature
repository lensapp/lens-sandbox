Feature: Signing in from the approval card
  A connector method whose mechanism is code decides for itself what to ask
  for and how many rounds it takes, so the values cannot be collected in one
  form and handed over at once. The approval card drives the exchange instead:
  it draws the round the mechanism is waiting on, sends the answer, and draws
  the next one, until the mechanism finishes and the card grants what it just
  disclosed. The words in a round come from code nobody can read, so the card
  says whose they are. A round nobody will answer is dropped rather than left
  holding what was already typed (sandbox-spec §3.2.6).

  Background:
    Given a run holds an offer for "some-provider" serving "*.some-provider.example"
    And the workload reaches "api.some-provider.example"

  Scenario: The first round of a sign-in reaches the card in the connector's name
    Given the mechanism asks for "device_code" saying "open the picker"
    When the developer starts connecting with "sign-in"
    Then the card shows the round asking for "device_code"
    And the card attributes "open the picker" to "some-provider"

  Scenario: A round that asks for nothing shows a message and a way to press on
    Given the mechanism asks for nothing saying "enter 8C29-9212 at github.com/login/device"
    When the developer starts connecting with "sign-in"
    Then the card shows a round with no fields
    And the card attributes "enter 8C29-9212 at github.com/login/device" to "some-provider"

  Scenario: Answering the last round connects and grants what the card disclosed
    Given the mechanism asks for "device_code" saying "open the picker"
    And the mechanism then finishes the connection "sign-in"
    When the developer starts connecting with "sign-in"
    And the developer answers the round with "device_code" as "8C29-9212"
    Then the mechanism was given "8C29-9212" for "device_code"
    Then the run holds a grant of "some-provider" through the connection "sign-in"
    And the workload's request proceeds
    And the card is gone

  Scenario: A mechanism that keeps asking is answered round after round
    Given the mechanism asks for "client_id" saying "which app is this"
    And the mechanism then asks for nothing saying "enter the code"
    And the mechanism then finishes the connection "sign-in"
    When the developer starts connecting with "sign-in"
    And the developer answers the round with "client_id" as "Iv23li"
    Then the card shows a round with no fields
    When the developer answers the round with nothing
    Then the run holds a grant of "some-provider" through the connection "sign-in"

  Scenario: A sign-in that fails says why and leaves the offer standing
    Given the mechanism asks for "device_code" saying "open the picker"
    And the mechanism then fails saying "GitHub returned no refresh token"
    When the developer starts connecting with "sign-in"
    And the developer answers the round with "device_code" as "8C29-9212"
    Then the developer is told "GitHub returned no refresh token"
    And the card carries no round
    And the run holds no grant of "some-provider"
    And the workload's request is still held

  Scenario: Closing the card mid-sign-in drops what was already typed
    Given the mechanism asks for "device_code" saying "open the picker"
    When the developer starts connecting with "sign-in"
    And the developer closes the card without choosing
    Then the mechanism's round was abandoned

  Scenario: A sign-in that ran out resets the card rather than answering nobody
    Given the mechanism asks for "device_code" saying "open the picker"
    And the round runs out before it is answered
    When the developer starts connecting with "sign-in"
    And the developer answers the round with "device_code" as "8C29-9212"
    Then the developer is told "that connect is no longer open; run it again"
    And the card carries no round
    And the run holds no grant of "some-provider"
    And the mechanism's round was abandoned

  Scenario: Answering the connector on another card drops the sign-in this one left open
    Given the workload also reaches "gist.some-provider.example"
    And the mechanism asks for "device_code" saying "open the picker"
    When the developer starts connecting with "sign-in"
    And the developer declines the connector on the other card
    Then the mechanism's round was abandoned

  Scenario: Starting a second sign-in drops the round the first one left open
    Given the mechanism asks for "device_code" saying "open the picker"
    And the mechanism then asks for "device_code" saying "open the picker"
    When the developer starts connecting with "sign-in"
    And the developer starts connecting with "sign-in"
    Then the mechanism's round was abandoned
