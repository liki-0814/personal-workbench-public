import fs from "fs"
import os from "os"
import path from "path"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

function readRuntimeConfig() {
  try {
    return JSON.parse(fs.readFileSync(path.join(os.homedir(), ".pwcli/config.json"), "utf8"))
  } catch {
    return {}
  }
}

export default defineConfig(() => {
  const runtime = readRuntimeConfig()
  const backendUrl = runtime.frontend?.backendUrl || ""
  const backendPort = Number(runtime.server?.backendPort) || 3456
  const proxy = {
    "/api": {
      target: backendUrl || `http://127.0.0.1:${backendPort}`,
      changeOrigin: true,
      ws: true,
    },
  }
  return {
    plugins: [react()],
    define: {
      __PWB_BACKEND_URL__: JSON.stringify(backendUrl),
    },
    resolve: {
      alias: {
        "@": path.resolve(__dirname, "../src"),
      },
    },
    server: {
      host: "127.0.0.1",
      port: 5173,
      strictPort: true,
      proxy,
    },
    preview: {
      host: "127.0.0.1",
      port: 5173,
      strictPort: true,
      proxy,
    },
    build: {
      rollupOptions: {
        input: {
          main: path.resolve(__dirname, "../index.html"),
        },
        output: {
          manualChunks(id) {
            if (!id.includes('node_modules')) return;
            if (id.includes('/react/') || id.includes('/react-dom/')) return 'vendor-react';
            if (id.includes('/framer-motion/')) return 'vendor-motion';
            if (id.includes('/sortablejs/')) return 'vendor-sortable';
            if (id.includes('/marked/')) return 'vendor-markdown';
            if (id.includes('/lucide-react/')) return 'vendor-lucide';
            if (id.includes('/monaco-editor/')) return 'vendor-monaco';
          },
        },
      },
    },
  }
})
