// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Helix, the Nebula Mail design system. The app imports UI only from here
// (never from Base UI directly). Tokens: "@nebulamail/ui/tokens.stylex".
// Font: "@nebulamail/ui/fonts.css".

export { Avatar, type AvatarProps, hueFor, initials } from "./avatar";
export {
  Badge,
  type BadgeProps,
  type BadgeTone,
  Tag,
  type TagProps,
} from "./badge";
export {
  AppIcon,
  type AppIconProps,
  NebulaLine,
  Mark,
  type MarkCore,
  type MarkProps,
  type MarkTone,
  ShieldGlyph,
  Spark,
  type SparkProps,
  sparkPath,
  Wordmark,
  type WordmarkProps,
} from "./brand";
export {
  Button,
  type ButtonProps,
  type ButtonSize,
  type ButtonVariant,
  ComposeButton,
  type ComposeButtonProps,
  type Icon,
  IconButton,
  type IconButtonProps,
} from "./button";
export { Checkbox, type CheckboxProps } from "./checkbox";
export {
  DateLocale,
  type DateLocaleProps,
  DatePicker,
  type DatePickerProps,
  type DayRange,
  MiniCalendar,
  type MiniCalendarProps,
  type MonthPickerLabels,
  TimeField,
  type TimeFieldProps,
  type WeekStart,
} from "./date-picker";
export {
  AlertDialog,
  type AlertDialogProps,
  Dialog,
  type DialogContentProps,
  type DialogRootProps,
} from "./dialog";
export { EmptyState, type EmptyStateProps } from "./empty-state";
export {
  CodeEditor,
  type CodeEditorProps,
  type CodeToken,
  type CodeTokenKind,
  DateInput,
  type DateInputProps,
  Select,
  type SelectEntry,
  type SelectGroup,
  type SelectOption,
  type SelectProps,
} from "./form-controls";
export {
  IllustrationContacts,
  IllustrationInboxZero,
  IllustrationNoResults,
  IllustrationOpenMail,
  IllustrationPrivacy,
} from "./illustrations";
export {
  Field,
  type FieldProps,
  Input,
  type InputProps,
  SearchField,
  type SearchFieldProps,
  Textarea,
  type TextareaProps,
} from "./input";
export {
  Kbd,
  type KbdProps,
  type KbdTone,
  Shortcut,
  type ShortcutProps,
} from "./kbd";
export {
  LinkButton,
  type LinkButtonProps,
  TextLink,
  type TextLinkProps,
} from "./link";
export {
  ContextMenu,
  type ContextMenuProps,
  Menu,
  MenuCheckboxItem,
  type MenuCheckboxItemProps,
  MenuGroup,
  type MenuGroupProps,
  MenuItem,
  type MenuItemProps,
  type MenuProps,
  MenuRadioGroup,
  type MenuRadioGroupProps,
  MenuRadioItem,
  type MenuRadioItemProps,
  MenuSeparator,
  MenuSubmenu,
  type MenuSubmenuProps,
} from "./menu";
export { NavItem, type NavItemProps } from "./nav-item";
export {
  Box,
  type BoxProps,
  Card,
  type CardProps,
  Panel,
  type PanelProps,
} from "./panel";
export {
  Popover,
  type PopoverContentProps,
  type PopoverRootProps,
} from "./popover";
export { PortalScope, type PortalScopeProps } from "./portal-scope";
export { PrivacyShield, type PrivacyShieldProps } from "./privacy-shield";
export {
  ProgressBar,
  type ProgressBarProps,
  ProgressRing,
  type ProgressRingProps,
} from "./progress";
export { UIProvider, type UIProviderProps } from "./provider";
export { isAllowedLink, normalizeLink } from "./editor-links";
export { EMOJI, EmojiPicker, type EmojiPickerProps } from "./emoji-picker";
export { type InsertedImage } from "./editor-schema";
export {
  PlainTextEditor,
  type PlainTextEditorProps,
} from "./plain-text-editor";
export {
  RichTextEditor,
  type RichTextEditorProps,
  type RichTextLabels,
  type RichTextShortcuts,
} from "./rich-text-editor";
export { ScrollArea, type ScrollAreaProps } from "./scroll-area";
export {
  SegmentedControl,
  type SegmentedControlProps,
  type SegmentedOption,
} from "./segmented";
export { Separator, type SeparatorProps } from "./separator";
export {
  type Suggestion,
  SuggestInput,
  type SuggestInputProps,
} from "./suggest-input";
export { Spinner, type SpinnerProps } from "./spinner";
export { Switch, type SwitchProps } from "./switch";
export {
  Tab,
  TabList,
  type TabListProps,
  TabPanel,
  type TabPanelProps,
  type TabProps,
  Tabs,
  type TabsProps,
} from "./tabs";
export { ThemeScope, type ThemeScopeProps } from "./theme";
export {
  darkTheme,
  type Density,
  densities,
  lightTheme,
  systemTheme,
  type ThemeMode,
  themeProps,
  themes,
} from "./themes";
export {
  type ToastApi,
  type ToastOptions,
  ToastProvider,
  type ToastProviderProps,
  type ToastTone,
  useToast,
} from "./toast";
export { Tooltip, type TooltipProps } from "./tooltip";
