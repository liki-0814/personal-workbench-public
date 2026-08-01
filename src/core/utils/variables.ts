// {{var}} 占位符解析与替换
// 规则：
//   - {{ name }} → 变量名为 "name"（首尾空白被 trim）
//   - 同一变量名多次出现，只算一个
//   - 变量名不能为空、不能含 { 或 }
//   - 替换时未提供值的变量 → 保留原始 {{name}}（提醒用户漏填）

const VAR_PATTERN = /\{\{\s*([^{}]+?)\s*\}\}/g;

export function extractVariables(content: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const m of content.matchAll(VAR_PATTERN)) {
    const name = m[1].trim();
    if (!name) continue;
    if (seen.has(name)) continue;
    seen.add(name);
    out.push(name);
  }
  return out;
}

export function substitute(content: string, values: Record<string, string>): string {
  return content.replace(VAR_PATTERN, (raw, name) => {
    const key = String(name).trim();
    if (!key) return raw;
    const v = values[key];
    return v !== undefined && v !== '' ? v : raw;
  });
}

export function hasVariables(content: string): boolean {
  return extractVariables(content).length > 0;
}
