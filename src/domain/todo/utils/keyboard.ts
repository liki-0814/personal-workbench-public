export function shouldSubmitOnEnter(key: string, isComposing: boolean): boolean {
  return key === 'Enter' && !isComposing;
}
