import { describe, expect, it } from 'vitest';
import { parseDataUrl } from './media';

describe('parseDataUrl', () => {
  it('parses base64 media URLs and rejects unrelated strings', () => {
    expect(parseDataUrl('data:image/png;base64,AAAA')).toEqual({ mediaType: 'image/png', data: 'AAAA' });
    expect(parseDataUrl('https://example.com/image.png')).toBeNull();
  });
});
