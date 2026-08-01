import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import './styles/precision.css'
import './shell/ui/markdown.css'
import 'katex/dist/katex.min.css'
import App from './App.tsx'
import { ErrorBoundary } from '@/shell'
import { syncToServer, syncFromServer } from '@/core/storage'
import { fetchLocalConfig } from '@/core/config'

// 启动同步策略：优先从后端拉取，确保前端能看到 agent/pwcli 做的修改。
// 用户主动修改已通过 save() 实时同步到后端，因此启动时不需要无条件推送。
// 仅当后端不可达时才推送本地数据作为备份。
async function bootstrapSync() {
  const pulled = await syncFromServer()
  if (!pulled) {
    await syncToServer()
  }
  // ~/.pwcli/config.json — nested config (moa / mineruToken /
  // tools.* / memory.*). Failure is non-fatal; SettingsModal will retry.
  await fetchLocalConfig().catch(() => { /* ignore */ })
}

bootstrapSync().then(() => {
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </StrictMode>,
  )
})
