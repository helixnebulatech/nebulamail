// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Safe email rendering. Browser entry point; servers should
// import `@nebulamail/mail-render/trackers` (DOM-free) and
// `@nebulamail/mail-render/extract` (needs an injected HTML parser).

export {
  DEFAULT_PRIVACY,
  shouldLoadRemoteImages,
  type DefaultView,
  type LinkConfirmation,
  type PrivacySettings,
  type RemoteImageContext,
  type RemoteImages,
} from "./settings";
export {
  TRUSTED_TYPES_POLICIES,
  type TrustedHtml,
  installTrustedTypes,
  parseInertHtml,
} from "./trusted-types";
// Not `mintRenderedFrame`: only render.ts makes frames.
export { type RenderedFrame, trustedSrcdoc } from "./rendered-frame";
export {
  renderEmail,
  type EmailContent,
  type FallbackReason,
  type RenderOptions,
  type RenderResult,
} from "./render";
export {
  type CidResolver,
  type ImageSource,
  type ImageUrlRewriter,
  validateLoadableUrl,
} from "./images";
export {
  BASE_STYLE,
  type TextScheme,
  InvalidImageSourceError,
  buildContentSecurityPolicy,
  imageSourcesFor,
  validateProxyOrigin,
} from "./document";
export {
  InvalidStyleNonceError,
  applyStyleNonce,
  validateStyleNonce,
} from "./nonce";
export {
  FRAME_CONTAIN,
  IFRAME_REFERRER_POLICY,
  IFRAME_SANDBOX,
  PRINT_IFRAME_SANDBOX,
  attachFrame,
  fillFramePage,
  findLinkFromEvent,
  type FrameHandlers,
  type FrameLink,
} from "./frame";
export {
  OPENABLE_PROTOCOLS,
  analyzeLink,
  cleanLink,
  domainsInText,
  needsConfirmation,
  parseMailto,
  registrableDomain,
  resolveLinkForOpening,
  type LinkAnalysis,
  type LinkWarning,
  type MailtoFields,
  type OpenableLink,
  type ResolveOptions,
} from "./links";
export {
  SPY_PIXEL_SERVICE,
  type CleanedLink,
  type PrivacyReport,
  type TrackerReportEntry,
} from "./report";
export { linkifySegments } from "./text";
export {
  DEFAULT_LIMITS,
  checkHtmlLimits,
  estimateNestingDepth,
  type LimitViolation,
  type RenderLimits,
} from "./limits";
export {
  BIDI_CONTROLS,
  INVISIBLE_CHARS,
  analyzeDisplayText,
  cleanDisplayText,
  hasBidiControls,
  stripBidiControls,
  stripInvisible,
  type DisplayTextResult,
} from "./unicode";
export {
  analyzeAttachment,
  isPreviewableImage,
  type AttachmentAnalysis,
  type AttachmentInput,
  type AttachmentWarning,
} from "./attachments";
export { SanitizerUnavailableError } from "./proton";
export {
  SAFE_CID,
  escapeHtml,
  sanitizeComposeHtml,
  textToComposeHtml,
  type ComposeHtmlOptions,
} from "./compose";
export {
  MAX_IMAGE_URL_LENGTH,
  cidUrl,
  classifyImageUrl,
  normalizeCid,
  normalizeContentId,
  type ImageUrlClass,
} from "./urls";
export {
  extractRemoteImageUrls,
  type ExtractOptions,
  type ExtractedImages,
  type HtmlParser,
} from "./extract";
export * from "./trackers/index";
export {
  COMPOSE_FONTS,
  COMPOSE_FONT_IDS,
  COMPOSE_FONT_SIZES,
  DEFAULT_COMPOSE_FONT,
  DEFAULT_COMPOSE_FONT_SIZE,
  TEXT_DIRECTIONS,
  composeFontOf,
  fontById,
  isComposeFontId,
  isComposeFontSize,
  isTextDirection,
  safeFontFamily,
  safeFontSize,
  withBodyStyle,
  type ComposeBodyStyle,
  type ComposeFont,
  type ComposeFontId,
  type ComposeFontSize,
  type TextDirection,
} from "./compose-style";
