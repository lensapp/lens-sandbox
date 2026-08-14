Feature: a mixin a document reads from a directory
  A document beside a working file may name a mixin by directory rather than by
  digest. The run reads that directory's own document and merges it exactly like
  a published one. Only a document that was itself read from disk may name a
  directory; a published one has no machine to read it on.

  A directory has no digest, so the run names it by absolute path. That is what
  the disclosure shows and what the walk treats as its identity.

  Scenario: a directory a local document declares reaches the run
    Given a mixin directory "/work/mixins/postgres-tools" declaring the tool "python@3.12"
    And the local definition at "/work" declares the mixin "./mixins/postgres-tools"
    When the local sandbox is resolved and launched
    Then the run installs "python@3.12"
    And the resolution names the mixin "/work/mixins/postgres-tools/lns.yaml"

  Scenario: a directory a published document names still refuses
    Given the sandbox definition declares the mixin "./mixins/postgres-tools"
    When the published sandbox is resolved and launched
    Then the launch is refused
    And the error says a published document cannot read a directory

  Scenario: a mixin's own directory reference roots where that mixin lives
    Given a mixin directory "/work/mixins/base" declaring the tool "node@22"
    And a mixin directory "/work/mixins/postgres-tools" declaring the mixin "../base"
    And the local definition at "/work" declares the mixin "./mixins/postgres-tools"
    When the local sandbox is resolved and launched
    Then the run installs "node@22"

  Scenario: two spellings of one directory are one identity
    Given a mixin directory "/work/mixins/postgres-tools" declaring the tool "python@3.12"
    And the local definition at "/work" declares the mixins "./mixins/postgres-tools" and "./mixins/../mixins/postgres-tools"
    When the local sandbox is resolved and launched
    Then the resolution names only the mixin "/work/mixins/postgres-tools/lns.yaml"
