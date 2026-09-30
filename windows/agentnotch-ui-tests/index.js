'use strict';
// `node --test agentnotch-ui-tests` (from windows/), the way both workflows run it.
//
// Node 22 and later treat that argument as a file, not as a folder to search, so the folder has
// to resolve as a module: this file. It loads every suite beside it; each registers its tests
// with node:test. (Node 20 finds the *.test.cjs files by itself and never loads this file.)
// One suite alone: `node --test agentnotch-ui-tests/panel.test.cjs`.

const fs = require('node:fs');
const path = require('node:path');

for (const name of fs.readdirSync(__dirname).filter((n) => n.endsWith('.test.cjs')).sort()) {
  require(path.join(__dirname, name));
}
