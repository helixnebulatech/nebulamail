// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Web Push for closed tabs, without a server of our
// own: the browser's push service gets the endpoint, the JMAP server gets
// a PushSubscription (RFC 8620 §7.2) with that endpoint and its keys, and
// pushes straight to it, signed with VAPID (the session's
// `urn:ietf:params:jmap:webpush-vapid` key) and encrypted (RFC 8291) so
// the push service can't read it. The service worker (pwa/sw.ts) receives
// it. Only for the types a feature asks for (calendar: "CalendarAlert");
// what each wants is remembered on this device.
//
// Verification: the server first pushes a PushVerification; the worker
// passes it to any open tab, which sends the code back
// (`listenForVerification`), or sends it itself when no tab is open and it
// can reach JMAP. Until verified the server pushes nothing else (RFC 8620
// §7.2.2), so a subscription still unverified a while after it was made
// (the code was lost: no tab, offline) is made again on the next start, as
// is one about to expire (Stalwart: 7 days) (`renewWebPush`).
//
// New mail ("EmailDelivery") on Stalwart 1.0 also asks for `emailPush`
// (mail/email-push.ts): the emails come inside the push.

import type { JmapClient } from "@nebulamail/jmap-client";
import { PushVerificationSchema, WEBPUSH_VAPID } from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import { prepareEmailPush } from "../mail/email-push";

const DEVICE_KEY = "wm-push-device";
const TYPES_KEY = "wm-push-types";
/** When this device last created its subscription (any tab). */
const SUBSCRIBED_KEY = "wm-push-subscribed-at";
/**
 * How long a new subscription may wait for its verification before it is
 * taken as lost: the push takes seconds; this also keeps two tabs starting
 * together from replacing each other's subscription.
 */
export const VERIFY_WAIT_MS = 5 * 60_000;
/** Renew when the subscription has less than this left. */
const RENEW_BEFORE_MS = 3 * 24 * 60 * 60_000;
const LIFETIME_MS = 7 * 24 * 60 * 60_000;

/** The type of new mail (mail/mail-push.ts `MAIL_PUSH_TYPE`). */
const MAIL_TYPE = "EmailDelivery";

/** Message the worker sends tabs (pwa/sw.ts, keep in sync). */
export const SW_PUSH_MESSAGE = "nebula-push";

export const webPushSupported = (): boolean =>
  typeof navigator !== "undefined" &&
  "serviceWorker" in navigator &&
  typeof PushManager !== "undefined" &&
  typeof Notification !== "undefined";

/** The server's VAPID key, if it can push to browsers. */
export const vapidKey = (client: JmapClient): string | null => {
  const capability: unknown = client.session.capabilities[WEBPUSH_VAPID];
  if (typeof capability !== "object" || capability === null) {
    return null;
  }
  const key: unknown = Reflect.get(capability, "applicationServerKey");
  return typeof key === "string" && /^[\w-]{20,200}$/u.test(key) ? key : null;
};

const readList = (key: string): string[] => {
  try {
    const value: unknown = JSON.parse(
      globalThis.localStorage.getItem(key) ?? "[]"
    );
    return Array.isArray(value)
      ? value.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
  }
};

/** The id made for this page load when it can't be kept in storage. */
let loadDevice: string | undefined;

const storedDevice = (): string | null => {
  try {
    const known = globalThis.localStorage.getItem(DEVICE_KEY);
    return known !== null && /^[\w-]{8,64}$/u.test(known) ? known : null;
  } catch {
    return null;
  }
};

/**
 * The `deviceClientId` of this browser (RFC 8620 §7.2). It must differ
 * between devices: a subscription from another session with the same id
 * replaces this one. So, without storage, a random id for this page load,
 * never a fixed one.
 */
export const deviceId = (): string => {
  const known = storedDevice();
  if (known !== null) {
    return known;
  }
  loadDevice ??= `nebula-${crypto.randomUUID()}`;
  try {
    globalThis.localStorage.setItem(DEVICE_KEY, loadDevice);
  } catch {
    // No storage: the id lasts until the page is loaded again.
  }
  return loadDevice;
};

const base64Url = (buffer: ArrayBuffer | null): string => {
  if (buffer === null) {
    return "";
  }
  let text = "";
  for (const byte of new Uint8Array(buffer)) {
    text += String.fromCodePoint(byte);
  }
  return btoa(text)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/[=]+$/u, "");
};

const fromBase64Url = (text: string): Uint8Array<ArrayBuffer> => {
  const padded = text.replaceAll("-", "+").replaceAll("_", "/");
  const binary = atob(padded + "=".repeat((4 - (padded.length % 4)) % 4));
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.codePointAt(index) ?? 0;
  }
  return bytes;
};

/** Whether a browser subscription was made for this VAPID key. */
export const sameKey = (
  push: Pick<PushSubscription, "options">,
  key: Uint8Array
): boolean => {
  const current = push.options.applicationServerKey;
  if (current === null) {
    return false;
  }
  const bytes = new Uint8Array(current);
  return (
    bytes.length === key.length &&
    bytes.every((byte, index) => byte === key[index])
  );
};

const registration = async (): Promise<ServiceWorkerRegistration | null> => {
  if (!webPushSupported()) {
    return null;
  }
  // Only production builds have a worker (pwa/service-worker.tsx), and its
  // scope is the app's base: "/nebulamail/" in the static build.
  const found = await navigator.serviceWorker.getRegistration(
    import.meta.env.BASE_URL
  );
  return found ?? null;
};

/** Our JMAP subscriptions for this device (normally one). */
const ourSubscriptions = async (client: JmapClient) => {
  const [result] = await client.request((b) => [
    b.call("PushSubscription/get", {}),
  ]);
  const device = deviceId();
  return result.list.filter((item) => item.deviceClientId === device);
};

const subscribe = async (
  client: JmapClient,
  types: readonly string[]
): Promise<boolean> => {
  const key = vapidKey(client);
  const worker = await registration();
  if (key === null || worker === null) {
    return false;
  }
  const serverKey = fromBase64Url(key);
  let push = await worker.pushManager.getSubscription();
  // A browser subscription made for another key (the server rotated its
  // VAPID key, or this is another server): the push service would refuse
  // the server's messages, so start again.
  if (push !== null && !sameKey(push, serverKey)) {
    await push.unsubscribe().catch(() => false);
    push = null;
  }
  push ??= await worker.pushManager.subscribe({
    applicationServerKey: serverKey,
    userVisibleOnly: true,
  });
  // The server can't show us the URL of a subscription, so ours are
  // replaced rather than compared.
  const old = await ourSubscriptions(client);
  const emailPush = types.includes(MAIL_TYPE)
    ? await prepareEmailPush(client).catch(() => null)
    : null;
  const create = {
    deviceClientId: deviceId(),
    expires: new Date(Date.now() + LIFETIME_MS)
      .toISOString()
      .replace(/\.\d{3}Z$/u, "Z"),
    keys: {
      auth: base64Url(push.getKey("auth")),
      p256dh: base64Url(push.getKey("p256dh")),
    },
    types: [...types],
    url: push.endpoint,
  };
  try {
    globalThis.localStorage.setItem(SUBSCRIBED_KEY, String(Date.now()));
  } catch {
    // Not remembered: an unverified one is then renewed on the next start.
  }
  const [result] = await client.request((b) => [
    b.call("PushSubscription/set", {
      create: {
        push: emailPush === null ? create : { ...create, emailPush },
      },
      ...(old.length === 0 ? {} : { destroy: old.map((item) => item.id) }),
    }),
  ]);
  if (result.created.push !== undefined || emailPush === null) {
    return result.created.push !== undefined;
  }
  // Refused with the mail inside: without it, as before (StateChange).
  const [again] = await client.request((b) => [
    b.call("PushSubscription/set", { create: { push: create } }),
  ]);
  return again.created.push !== undefined;
};

const unsubscribe = async (client: JmapClient | null): Promise<void> => {
  if (client !== null) {
    const old = await ourSubscriptions(client).catch(() => []);
    if (old.length > 0) {
      await client
        .request((b) => [
          b.call("PushSubscription/set", {
            destroy: old.map((item) => item.id),
          }),
        ])
        .catch(() => null);
    }
  }
  const worker = await registration().catch(() => null);
  const push = await worker?.pushManager.getSubscription();
  await push?.unsubscribe().catch(() => false);
};

/** The push types wanted on this device. */
export const wantedPushTypes = (): string[] => readList(TYPES_KEY);

const saveTypes = (types: readonly string[]): void => {
  try {
    globalThis.localStorage.setItem(TYPES_KEY, JSON.stringify([...types]));
  } catch {
    // Not remembered: asked again next time.
  }
};

/** Whether this server and browser can push to closed tabs. */
export const webPushAvailable = async (): Promise<boolean> => {
  if (!webPushSupported()) {
    return false;
  }
  const client = await getClient();
  return vapidKey(client) !== null && (await registration()) !== null;
};

/**
 * Subscribing, renewing and forgetting run one after another in this tab:
 * two at once (turning a setting on while the start-up renewal runs) would
 * each create a subscription, and the second would destroy the first,
 * whose verification is on its way.
 */
let queue: Promise<unknown> = Promise.resolve();
const oneAtATime = async <T>(work: () => Promise<T>): Promise<T> => {
  const next = queue.then(work, work);
  queue = next.catch(() => null);
  return await next;
};

const applyPushTypes = async (change: {
  readonly add?: readonly string[];
  readonly remove?: readonly string[];
}): Promise<boolean> => {
  const next = new Set(wantedPushTypes());
  for (const type of change.add ?? []) {
    next.add(type);
  }
  for (const type of change.remove ?? []) {
    next.delete(type);
  }
  saveTypes([...next]);
  const client = await getClient();
  if (next.size === 0) {
    await unsubscribe(client);
    return false;
  }
  return await subscribe(client, [...next]);
};

/**
 * Adds or removes push types for this device and updates the server's
 * subscription. Resolves to whether pushes reach this device afterwards.
 */
export const setPushTypes = async (change: {
  readonly add?: readonly string[];
  readonly remove?: readonly string[];
}): Promise<boolean> =>
  await oneAtATime(async () => await applyPushTypes(change));

const subscribedAt = (): number => {
  try {
    const value = Number(globalThis.localStorage.getItem(SUBSCRIBED_KEY));
    return Number.isFinite(value) ? value : 0;
  } catch {
    return 0;
  }
};

/**
 * Whether this device's subscription must be made again: none, lost by the
 * browser, about to expire, or still unverified (RFC 8620 §7.2: the
 * server returns `verificationCode` null until the client sets it) well
 * after it was made.
 */
export const needsRenewal = (
  current:
    | {
        readonly expires?: string | null | undefined;
        readonly verificationCode?: string | null | undefined;
      }
    | undefined,
  lost: boolean,
  now: number,
  lastSubscribed: number
): boolean => {
  if (current === undefined || lost) {
    return true;
  }
  const expires = Date.parse(current.expires ?? "");
  if (!Number.isFinite(expires) || expires - now < RENEW_BEFORE_MS) {
    return true;
  }
  const unverified =
    current.verificationCode === null || current.verificationCode === undefined;
  return unverified && now - lastSubscribed >= VERIFY_WAIT_MS;
};

const renew = async (): Promise<void> => {
  const types = wantedPushTypes();
  if (types.length === 0 || !webPushSupported()) {
    return;
  }
  const worker = await registration();
  if (worker === null || Notification.permission !== "granted") {
    return;
  }
  const client = await getClient();
  const [current] = await ourSubscriptions(client);
  const lost = (await worker.pushManager.getSubscription()) === null;
  if (needsRenewal(current, lost, Date.now(), subscribedAt())) {
    await subscribe(client, types);
  }
};

/** On start: renew a subscription about to expire, lost or never verified. */
export const renewWebPush = async (): Promise<void> => {
  await oneAtATime(renew);
};

/** Signing out: the server stops pushing here and the endpoint goes. */
export const forgetWebPush = async (): Promise<void> => {
  if (!webPushSupported()) {
    return;
  }
  await oneAtATime(async () => {
    saveTypes([]);
    const client = await getClient().catch(() => null);
    await unsubscribe(client);
  });
};

const verify = async (
  pushSubscriptionId: string,
  verificationCode: string
): Promise<void> => {
  const client = await getClient();
  await client.request((b) => [
    b.call("PushSubscription/set", {
      update: { [pushSubscriptionId]: { verificationCode } },
    }),
  ]);
};

const onWorkerMessage = (event: MessageEvent): void => {
  const data: unknown = event.data;
  if (
    typeof data !== "object" ||
    data === null ||
    Reflect.get(data, "type") !== SW_PUSH_MESSAGE
  ) {
    return;
  }
  const parsed = PushVerificationSchema.safeParse(Reflect.get(data, "object"));
  if (parsed.success) {
    const { pushSubscriptionId, verificationCode } = parsed.data;
    verify(pushSubscriptionId, verificationCode).catch(() => null);
  }
};

/**
 * Sends back verification codes the worker passes on. Any open tab may
 * do it: the code only proves the endpoint is ours.
 */
export const listenForVerification = (): (() => void) => {
  if (!webPushSupported()) {
    return () => {
      // Nothing to stop.
    };
  }
  navigator.serviceWorker.addEventListener("message", onWorkerMessage);
  return () => {
    navigator.serviceWorker.removeEventListener("message", onWorkerMessage);
  };
};
