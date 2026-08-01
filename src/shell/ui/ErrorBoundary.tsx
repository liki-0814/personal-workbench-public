import { Component, type ReactNode } from 'react';
import { AlertTriangle, RotateCcw, Trash2 } from 'lucide-react';

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: React.ErrorInfo) {
    console.error('App crashed:', error, errorInfo);
  }

  handleReset = () => {
    this.setState({ hasError: false, error: null });
  };

  handleClearData = () => {
    const keys = Object.keys(localStorage).filter(key => key.startsWith('pwb_'));
    // eslint-disable-next-line no-restricted-syntax -- emergency reset: wipe local cache without backend round-trip
    keys.forEach(key => localStorage.removeItem(key));
    window.location.reload();
  };

  render() {
    if (this.state.hasError) {
      return (
        <div className="error-screen">
          <div className="error-card">
            <div className="error-card-icon"><AlertTriangle size={20} /></div>
            <span className="error-card-kicker">Application error</span>
            <h2>页面无法继续运行</h2>
            <p>
              {this.state.error?.message || '发生了未知错误'}
            </p>
            <div className="error-card-actions">
              <button
                onClick={this.handleReset}
                className="error-card-retry"
              >
                <RotateCcw size={14} />
                重试
              </button>
              <button
                onClick={this.handleClearData}
                className="error-card-reset"
              >
                <Trash2 size={14} />
                清除数据并重置
              </button>
            </div>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
