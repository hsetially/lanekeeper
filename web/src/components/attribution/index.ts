// The shared pieces that say who changed a file and how serious a change is. The chip and its parts are for history entries, drift
// rows, the activity feed, audit rows and the head of a file; the severity badge, the severity filter and the severity helpers are
// the ONE implementation of severity for every screen.
export { AttributionChip, type AttributionChipProps } from "./AttributionChip";
export { AttributionEvidence, type AttributionEvidenceProps } from "./AttributionEvidence";
export { AttributionSourceCell } from "./AttributionSourceCell";
export { ConfidenceMeter, type ConfidenceMeterProps } from "./ConfidenceMeter";
export { UnknownAuthorBadge } from "./UnknownAuthorBadge";
export {
  CONFIDENCE_LEVELS,
  SOURCE_LABEL,
  SOURCE_MEANING,
  confidenceView,
  describeAttribution,
  evidenceRows,
  shortUserId,
  shownConfidence,
  userLabel,
  type AttributionView,
  type AttributionWho,
  type ConfidenceView,
  type EvidenceRow,
  type NameLookup,
  type WhoContext,
  type WhoKind,
} from "./model";
export { SeverityBadge, type SeverityBadgeProps } from "./SeverityBadge";
export { SeverityFilter, type SeverityFilterProps } from "./SeverityFilter";
export {
  MIN_SEVERITY_CHOICES,
  SEVERITIES,
  SEVERITY_LABEL,
  atLeast,
  compareSeverity,
  filterAtLeast,
  loadedScopeNote,
  minSeverityLabel,
  minSeverityParam,
  parseSeverity,
  severityRank,
  sortBySeverity,
  type MinSeverityChoice,
} from "./severity";
