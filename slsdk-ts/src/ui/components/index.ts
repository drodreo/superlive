/**
 * Component facility barrel exports (@shell/sdk/ui).
 * Categories: list / form / dialog / state / general (Button·Select·Checkbox).
 * The query facility (query/) does not go through this barrel; business sides import directly from the subpath (original convention kept).
 */

// list
export { DataTable, type DataTableProps, type DataTableColumn } from './DataTable';
export { Pagination, type PaginationProps } from './Pagination';
export { SearchBar, type SearchBarProps } from './SearchBar';

// form
export { Input, type InputProps } from './Input';
export { Textarea, type TextareaProps } from './Textarea';
export { FormField, type FormFieldProps, type FormFieldSchema, type FormSchema, type FileAutoFill } from './FormField';

// dialog
export { Modal, type ModalProps } from './Modal';
export { ConfirmDialog, type ConfirmDialogProps } from './ConfirmDialog';
export { FormDialog, type FormDialogProps } from './FormDialog';

// state
export { LoadingSpinner, type LoadingSpinnerProps } from './LoadingSpinner';
export { EmptyState, type EmptyStateProps, type EmptyStateIcon } from './EmptyState';
export { ErrorMessage, type ErrorMessageProps } from './ErrorMessage';

// general
export { Button, type ButtonProps, type ButtonVariant, type ButtonSize } from './Button';
export { Select, type SelectProps } from './Select';
export { Checkbox, type CheckboxProps } from './Checkbox';
