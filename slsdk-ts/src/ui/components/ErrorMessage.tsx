/**
 * ErrorMessage - error display component (verbatim from superlive; the copy was always Chinese).
 */
import { Button } from './Button';

export interface ErrorMessageProps {
  error: string | Error | null;
  onRetry?: () => void;
  title?: string;
}

export function ErrorMessage({ error, onRetry, title = '出错了' }: ErrorMessageProps) {
  const message = error instanceof Error ? error.message : String(error ?? '未知错误');

  return (
    <div className="sl-ui-error-message">
      <span className="sl-ui-error-icon">⚠️</span>
      <h3 className="sl-ui-error-title">{title}</h3>
      <p className="sl-ui-error-description">{message}</p>
      {onRetry && (
        <Button variant="danger" size="md" onClick={onRetry}>
          重试
        </Button>
      )}
    </div>
  );
}

export default ErrorMessage;
