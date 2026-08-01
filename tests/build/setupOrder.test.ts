import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

describe('production setup build order', () => {
  it('builds Web assets before compiling the binary that embeds them', () => {
    const setup = readFileSync(resolve(process.cwd(), 'setup.sh'), 'utf8');
    const webBuild = setup.indexOf('npm run build --silent');
    const rustBuild = setup.indexOf('(cd pwcli && cargo build --release --quiet)');

    expect(webBuild).toBeGreaterThan(-1);
    expect(rustBuild).toBeGreaterThan(webBuild);
  });
});
