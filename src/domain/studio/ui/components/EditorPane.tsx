import { useCallback, useRef, useEffect } from 'react';
import Editor, { type BeforeMount, type OnMount } from '@monaco-editor/react';
import type { editor } from 'monaco-editor';

interface EditorPaneProps {
  content: string;
  language: string;
  isDark: boolean;
  onChange: (value: string) => void;
  onSave: () => void;
  readOnly?: boolean;
}

const configureThemes: BeforeMount = (monaco) => {
  monaco.editor.defineTheme('pwb-precision-light', {
    base: 'vs',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#FFFFFF',
      'editor.foreground': '#171A1F',
      'editor.lineHighlightBackground': '#F4F5F7',
      'editor.lineHighlightBorder': '#00000000',
      'editor.selectionBackground': '#3559D633',
      'editor.inactiveSelectionBackground': '#3559D61F',
      'editorCursor.foreground': '#3559D6',
      'editorLineNumber.foreground': '#9CA3AF',
      'editorLineNumber.activeForeground': '#374151',
      'editorIndentGuide.background1': '#E5E7EB',
      'editorIndentGuide.activeBackground1': '#D1D5DB',
      'editorWidget.background': '#FFFFFF',
      'editorWidget.border': '#D9DDE4',
    },
  });
  monaco.editor.defineTheme('pwb-precision-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [],
    colors: {
      'editor.background': '#14171C',
      'editor.foreground': '#E9ECF1',
      'editor.lineHighlightBackground': '#1A1E25',
      'editor.lineHighlightBorder': '#00000000',
      'editor.selectionBackground': '#7B96FF33',
      'editor.inactiveSelectionBackground': '#7B96FF1F',
      'editorCursor.foreground': '#7B96FF',
      'editorLineNumber.foreground': '#6B7280',
      'editorLineNumber.activeForeground': '#D1D5DB',
      'editorIndentGuide.background1': '#2A3039',
      'editorIndentGuide.activeBackground1': '#4B5563',
      'editorWidget.background': '#1A1E25',
      'editorWidget.border': '#2A3039',
    },
  });
};

export default function EditorPane({ content, language, isDark, onChange, onSave, readOnly = false }: EditorPaneProps) {
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);

  const handleMount: OnMount = useCallback((ed) => {
    editorRef.current = ed;
    ed.addCommand(2097, () => onSave()); // KeyMod.CtrlCmd | KeyCode.KeyS
  }, [onSave]);

  useEffect(() => {
    return () => { editorRef.current = null; };
  }, []);

  return (
    <Editor
      height="100%"
      language={language}
      value={content}
      theme={isDark ? 'pwb-precision-dark' : 'pwb-precision-light'}
      beforeMount={configureThemes}
      onChange={(v) => onChange(v ?? '')}
      onMount={handleMount}
      options={{
        fontFamily: '"SFMono-Regular", "SF Mono", ui-monospace, monospace',
        fontSize: 13,
        lineHeight: 21,
        minimap: { enabled: false },
        scrollBeyondLastLine: false,
        wordWrap: language === 'markdown' ? 'on' : 'off',
        padding: { top: 14 },
        renderWhitespace: 'selection',
        bracketPairColorization: { enabled: true },
        smoothScrolling: false,
        cursorSmoothCaretAnimation: 'off',
        readOnly,
      }}
    />
  );
}
