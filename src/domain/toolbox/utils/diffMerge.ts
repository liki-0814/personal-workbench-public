import { diffLines } from 'diff';

export interface MergeBlock {
  id: string;
  kind: 'equal' | 'change';
  left: string;
  right: string;
  leftLine: number;
  rightLine: number;
}

export function buildMergeBlocks(
  original: string,
  changed: string,
  ignoreWhitespace = false,
): MergeBlock[] {
  const changes = diffLines(original, changed, { ignoreWhitespace });
  const blocks: MergeBlock[] = [];
  let leftLine = 1;
  let rightLine = 1;
  let changeIndex = 0;

  for (let index = 0; index < changes.length;) {
    const part = changes[index];
    if (!part.added && !part.removed) {
      blocks.push({
        id: `equal-${blocks.length}`,
        kind: 'equal',
        left: part.value,
        right: part.value,
        leftLine,
        rightLine,
      });
      leftLine += part.count ?? 0;
      rightLine += part.count ?? 0;
      index += 1;
      continue;
    }

    const startLeftLine = leftLine;
    const startRightLine = rightLine;
    let left = '';
    let right = '';
    while (index < changes.length && (changes[index].added || changes[index].removed)) {
      const change = changes[index];
      if (change.removed) {
        left += change.value;
        leftLine += change.count ?? 0;
      } else if (change.added) {
        right += change.value;
        rightLine += change.count ?? 0;
      }
      index += 1;
    }
    blocks.push({
      id: `change-${changeIndex}`,
      kind: 'change',
      left,
      right,
      leftLine: startLeftLine,
      rightLine: startRightLine,
    });
    changeIndex += 1;
  }

  return blocks;
}

export function mergeFromChoices(
  blocks: MergeBlock[],
  choices: Record<string, 'left' | 'right'>,
): string {
  return blocks
    .map(block => {
      if (block.kind === 'equal') return block.right;
      return choices[block.id] === 'left' ? block.left : block.right;
    })
    .join('');
}

export function changeBlocks(blocks: MergeBlock[]): MergeBlock[] {
  return blocks.filter(block => block.kind === 'change');
}
