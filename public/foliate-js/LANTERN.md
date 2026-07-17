# Lantern vendored Foliate.js

This directory is a vendored copy of Foliate.js used by Lantern's reader. It was
converted from the `KlaraGraff/foliate-js` commit
`112eb278e4fc04f48f494dd71b213b2536bf4062` on 2026-07-17 so a Lantern checkout
does not require a Git submodule.

The original project remains MIT-licensed; see `LICENSE`. This copy includes
Lantern's iframe-load reliability fixes formerly maintained on
`lantern/iframe-load-timeout`. Make future reader-engine updates directly in
this directory and commit them with the Lantern change that requires them.
