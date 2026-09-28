/** Stable public imports for status-to-UI presentation rules. */
export {
  bargeInPresence,
  bargeInStage,
  humanizeActivity,
  isConfirmContext,
  isToolActivity,
} from "./status/activity";
export { conversationLines, pickCaption, pickSecondary } from "./status/conversation";
export { dedupeSecondary } from "./status/labels";
export {
  canCollapseToOrb,
  overlayInputUsesCard,
  overlayStageMode,
  overlayThinkingText,
  pickOverlayPresence,
  shouldShowOverlayCard,
  shouldStayExpanded,
} from "./status/overlay";
export type {
  BargeInStage,
  Caption,
  CaptionKind,
  ConversationLine,
  OverlayPresence,
  OverlayStageMode,
} from "./status/types";
