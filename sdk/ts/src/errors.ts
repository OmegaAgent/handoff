/**
 * The exception taxonomy, mirroring protocol §13.
 *
 * Every error the server returns uses one envelope and carries a stable machine-readable `code`.
 * Branch on the code — or on the class, which is the same thing with better ergonomics — and never
 * on `message`, which is written for people and may change at any time.
 *
 * An unrecognized code throws `HandoffProtocolError` with the code intact rather than being
 * coerced into the nearest familiar class. Guessing at an error you do not understand is how a
 * client silently mishandles a state the server is telling it about (I21).
 */

export interface FieldError {
  /** The declared field name that failed. */
  name: string;
  /** Stable machine-readable reason: `required`, `not_an_option`, `out_of_range`, … */
  code: string;
  message?: string;
}

export interface ErrorDetail {
  code?: string;
  status?: number;
  requestId?: string | null;
  receiptId?: string | null;
  supersededBy?: string | null;
  fields?: FieldError[];
  docs?: string;
  retryAfter?: number;
}

/**
 * Base class for everything this SDK throws against a Handoff server. Never carries a credential:
 * the message is the server's own text plus identifiers, all of which are ordinary data (§1.4).
 */
export class HandoffError extends Error {
  readonly code: string;
  readonly status?: number;
  readonly requestId?: string | null;
  readonly receiptId?: string | null;
  readonly supersededBy?: string | null;
  readonly fields: FieldError[];
  readonly docs?: string;
  readonly retryAfter?: number;

  constructor(message: string, detail: ErrorDetail = {}) {
    super(message);
    this.name = new.target.name;
    this.code = detail.code ?? "";
    this.status = detail.status;
    this.requestId = detail.requestId;
    this.receiptId = detail.receiptId;
    this.supersededBy = detail.supersededBy;
    this.fields = detail.fields ?? [];
    this.docs = detail.docs;
    this.retryAfter = detail.retryAfter;
  }
}

/** The server returned an error code this SDK version does not recognize. Fail closed (§19, I21). */
export class HandoffProtocolError extends HandoffError {}
/** The server could not be reached, or answered with something that was not the envelope. */
export class TransportError extends HandoffError {}

export class InvalidRequest extends HandoffError {}
/** A declared answer field type this server will not accept. Nothing was created (§5.3). */
export class UnsupportedFieldType extends HandoffError {}
export class UnsupportedCapabilityType extends HandoffError {}
/** The `requires.v` envelope version is not implemented here. No request exists (§5.2, C-16). */
export class UnsupportedRequiresVersion extends HandoffError {}
/** Absent, malformed, revoked, or expired credentials — deliberately one code (§13). */
export class AuthenticationError extends HandoffError {}
export class InsufficientScope extends HandoffError {}
export class NotEntitled extends HandoffError {}
/** The answerer did not meet the authority the request declared (§4.3, §4.4). */
export class InsufficientAuthority extends HandoffError {}
/** A machine principal tried to answer. Enforced by principal type and by nothing else (§4.2). */
export class RequesterMayNotAnswer extends HandoffError {}
export class TenantMismatch extends HandoffError {}
export class AuthStrengthNotPermitted extends HandoffError {}
/** Returned instead of 403 wherever existence is itself sensitive (§3.2). */
export class NotFound extends HandoffError {}
export class RequestNotFound extends NotFound {}
export class CapabilityNotFound extends NotFound {}
export class SignalNotFound extends NotFound {}
export class AuthorizationNotFound extends NotFound {}
/** A person already settled this. `receiptId` names the decision that exists (§6.7, I5). */
export class AlreadyAnswered extends HandoffError {}
export class RequestExpired extends HandoffError {}
export class RequestCancelled extends HandoffError {}
/** `supersededBy` names where to send the person instead (§6.5). */
export class RequestSuperseded extends HandoffError {}
/** Somebody has begun answering; amend is refused and the caller must supersede (§6.2 R2). */
export class RequestInProgress extends HandoffError {}
/** Same key, different body. The stored request was not modified (§3.3). */
export class IdempotencyKeyReused extends HandoffError {}
/** A single-use authorization was redeemed with a different `effect_key` (§10, I10). */
export class AuthorizationSpent extends HandoffError {}
/** The effect's shape disagrees with what was authorized (§10). */
export class EffectDigestMismatch extends HandoffError {}
export class BlastRadiusMismatch extends HandoffError {}
export class GrantAlreadyHeld extends HandoffError {}
/** The answer was against wording the person is no longer being shown (§9.3). */
export class PresentationStale extends HandoffError {}
export class CapabilityExpired extends HandoffError {}
/** Carries per-field detail in `.fields` (§5.3, §13). */
export class AnswerValidationFailed extends HandoffError {}
export class RateLimited extends HandoffError {}
/** The request exists and the ladder will retry. A channel outage never loses the ask (§7.3). */
export class DeliveryUnavailable extends HandoffError {}

/**
 * The caller's own deadline passed with no terminal signal.
 *
 * A local deadline, not a protocol outcome. The durable wait is still on the server and the
 * request may still be answered; reattach to pick it up. Where "nobody answered" must be an
 * outcome, declare it at raise time with `ttl_policy` (§6.4) so the record says a policy decided.
 */
export class HandoffTimeout extends Error {
  readonly waiterRef?: string;
  readonly requestId?: string;

  constructor(message: string, detail: { waiterRef?: string; requestId?: string } = {}) {
    super(message);
    this.name = "HandoffTimeout";
    this.waiterRef = detail.waiterRef;
    this.requestId = detail.requestId;
  }
}

/**
 * Thrown inside a `receive()` block to record that the decision could not be applied.
 *
 * The signal is acked with `applied: false` and the reason, which stops redelivery and records
 * the fact (§8.3). It is not swallowed and it is not an error the server rejects.
 */
export class SignalNotApplied extends Error {
  readonly reason: string;

  constructor(reason: string) {
    super(reason);
    this.name = "SignalNotApplied";
    this.reason = reason;
  }
}

/**
 * An inbound callback failed verification (§15, signing.md §1.3).
 *
 * The message names which check failed and never includes a secret or any value derived from one.
 */
export class CallbackSignatureError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CallbackSignatureError";
  }
}

const BY_CODE: Record<string, new (message: string, detail?: ErrorDetail) => HandoffError> = {
  invalid_request: InvalidRequest,
  unsupported_field_type: UnsupportedFieldType,
  unsupported_capability_type: UnsupportedCapabilityType,
  unsupported_requires_version: UnsupportedRequiresVersion,
  invalid_api_key: AuthenticationError,
  authentication_required: AuthenticationError,
  insufficient_scope: InsufficientScope,
  product_not_entitled: NotEntitled,
  insufficient_authority: InsufficientAuthority,
  requester_may_not_answer: RequesterMayNotAnswer,
  tenant_mismatch: TenantMismatch,
  auth_strength_not_permitted: AuthStrengthNotPermitted,
  request_not_found: RequestNotFound,
  capability_not_found: CapabilityNotFound,
  signal_not_found: SignalNotFound,
  authorization_not_found: AuthorizationNotFound,
  already_answered: AlreadyAnswered,
  request_expired: RequestExpired,
  request_cancelled: RequestCancelled,
  request_superseded: RequestSuperseded,
  request_in_progress: RequestInProgress,
  idempotency_key_reused: IdempotencyKeyReused,
  authorization_spent: AuthorizationSpent,
  effect_digest_mismatch: EffectDigestMismatch,
  blast_radius_mismatch: BlastRadiusMismatch,
  grant_already_held: GrantAlreadyHeld,
  presentation_stale: PresentationStale,
  capability_expired: CapabilityExpired,
  answer_validation_failed: AnswerValidationFailed,
  rate_limited: RateLimited,
  delivery_unavailable: DeliveryUnavailable,
};

/** Build the right error from the protocol's single error envelope (§13). */
export function fromErrorBody(
  body: any,
  detail: { status?: number; retryAfter?: number } = {},
): HandoffError {
  const error = body && typeof body === "object" && body.error ? body.error : {};
  const code = typeof error.code === "string" ? error.code : "";
  const message = typeof error.message === "string" ? error.message : `HTTP ${detail.status}`;
  const Cls = BY_CODE[code] ?? HandoffProtocolError;
  return new Cls(message, {
    code,
    status: detail.status,
    requestId: error.request_id ?? undefined,
    receiptId: error.receipt_id ?? undefined,
    supersededBy: error.superseded_by ?? undefined,
    fields: Array.isArray(error.fields) ? error.fields : [],
    docs: error.docs,
    retryAfter: detail.retryAfter,
  });
}
