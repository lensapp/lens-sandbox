Feature: a user adds their own mixins to a run
  A run takes mixins the user names on the command line, on top of the ones the
  document declares. They are the user's own live choice, so they come last and
  beat everything the document said.

  A reference the user types may be a tag, where a published document's may not.
  The run pins it and reports the digest it resolved to, so what the user
  approves names exact bytes.

  Scenario: what the user adds reaches the run
    Given a mixin declaring the tool "python@3.12"
    When the published sandbox is resolved with that mixin added by the user
    Then the run installs "python@3.12"

  Scenario: what the user adds beats what the document declares
    Given a mixin declaring the tool "node@22"
    And the sandbox definition declares the tool "node@20"
    When the published sandbox is resolved with that mixin added by the user
    Then the run installs "node@22"

  Scenario: a tag the user names is pinned before the run reports it
    Given a mixin declaring the tool "python@3.12" published under the tag "obs-tools:2"
    When the published sandbox is resolved with the user's mixin "obs-tools:2"
    Then the resolution reports the mixin pinned by digest
    And the resolution answers for the tag "obs-tools:2"

  Scenario: a directory named for a published sandbox refuses the run
    When the published sandbox is resolved with the user's mixin "./obs-tools"
    Then the launch is refused
    And the error says a directory merges only into a document this machine read
