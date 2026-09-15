Feature: Signed app bundles are managed as a whole
  Scenario Outline: A helper cannot replace or remove individual signed binaries
    Given the CLI executable is "<path>"
    When loose-binary installation management is checked
    Then installation management requires replacing or removing the complete app

    Examples:
      | path                                                      |
      | /Applications/LNS.app/Contents/Helpers/lns                  |
      | /Users/developer/My Apps/LNS.app/Contents/Helpers/lns       |
      | /Applications/LNS.app/Contents/MacOS/lns                   |

  Scenario Outline: Loose binaries retain normal installation management
    Given the CLI executable is "<path>"
    When loose-binary installation management is checked
    Then loose-binary installation management is allowed

    Examples:
      | path                              |
      | /Users/developer/.local/bin/lns    |
      | /work/target/debug/lns             |
      | /work/project.app/target/lns       |
      | /work/Contents/Helpers/lns         |
      | lns                              |
