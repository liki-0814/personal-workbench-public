import { describe, expect, it } from 'vitest';
import { buildMergeBlocks, changeBlocks, mergeFromChoices } from '@/domain/toolbox/utils/diffMerge';
import { parseTimestamp } from '@/domain/toolbox/utils/timestamp';

describe('toolbox diff and merge', () => {
  it('groups replacement lines into one selectable change', () => {
    const blocks = buildMergeBlocks('one\ntwo\nthree\n', 'one\nTWO\nthree\n');
    const changes = changeBlocks(blocks);

    expect(changes).toHaveLength(1);
    expect(changes[0].left).toBe('two\n');
    expect(changes[0].right).toBe('TWO\n');
    expect(mergeFromChoices(blocks, {})).toBe('one\nTWO\nthree\n');
    expect(mergeFromChoices(blocks, { [changes[0].id]: 'left' })).toBe('one\ntwo\nthree\n');
  });

  it('can ignore whitespace-only changes', () => {
    const blocks = buildMergeBlocks('const x = 1;\n', '  const x = 1;  \n', true);
    expect(changeBlocks(blocks)).toHaveLength(0);
  });
});

describe('timestamp conversion', () => {
  it('auto-detects seconds and milliseconds', () => {
    expect(parseTimestamp('1700000000', 'auto')?.milliseconds).toBe('1700000000000');
    expect(parseTimestamp('1700000000000', 'auto')?.seconds).toBe('1700000000');
  });

  it('supports microseconds and rejects invalid text', () => {
    expect(parseTimestamp('1700000000000000', 'microseconds')?.milliseconds).toBe('1700000000000');
    expect(parseTimestamp('not-a-timestamp', 'auto')).toBeNull();
  });
});
