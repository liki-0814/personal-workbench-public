export interface DiffLine {
  text: string;
  kind: 'add' | 'delete' | 'hunk' | 'meta' | 'context';
  oldLine?: number;
  newLine?: number;
}

export function parseUnifiedPatch(patch: string): DiffLine[] {
  let oldLine: number | undefined;
  let newLine: number | undefined;
  return patch.split('\n').map(text => {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text);
    if (hunk) {
      oldLine = Number(hunk[1]);
      newLine = Number(hunk[2]);
      return { text, kind: 'hunk' };
    }
    if (text.startsWith('diff ') || text.startsWith('index ') || text.startsWith('---') || text.startsWith('+++')) {
      return { text, kind: 'meta' };
    }
    if (text.startsWith('+')) {
      const line = { text, kind: 'add' as const, newLine };
      if (newLine != null) newLine += 1;
      return line;
    }
    if (text.startsWith('-')) {
      const line = { text, kind: 'delete' as const, oldLine };
      if (oldLine != null) oldLine += 1;
      return line;
    }
    const line = { text, kind: 'context' as const, oldLine, newLine };
    if (oldLine != null) oldLine += 1;
    if (newLine != null) newLine += 1;
    return line;
  });
}
