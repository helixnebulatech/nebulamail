// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings › General › Privacy: remote images (when and
// how, within what the admin allows, mail/image-policy.ts; the static
// build has its own choice), read receipts (mail/read-receipts.ts, only if
// the server has MDN) and "Download images when an email arrives"
// only when the image proxy has `preload`. The
// preload choice is kept by the proxy (keyed by hashes of the user's
// addresses), not on the device: the proxy is what acts on it when mail
// arrives. A full preload quota (accepted risk H15) is shown under the
// switch, with the date it frees up (mail/preload-status.ts).

import { Button, SegmentedControl, Switch } from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { ImageLoadMode, ImageRoute } from "../../mail/image-policy";
import { useOneClickUnsubscribe } from "../../mail/one-click-unsubscribe";
import type { PreloadFull, PreloadState } from "../../mail/preload-status";
import {
  postPreload,
  preloadStatusKey,
  usePreloadStatus,
} from "../../mail/preload-status";
import { READ_RECEIPT_MODES } from "../../mail/read-receipts";
import type { ReadReceiptMode } from "../../mail/read-receipts";
import type { RemoteImageMode } from "../../mail/remote-images";
import {
  AVAILABLE_REMOTE_IMAGE_MODES,
  useRemoteImageMode,
} from "../../mail/remote-images";
import { useImageSettings } from "../../mail/use-image-policy";
import {
  useReadReceiptMode,
  useReceiptsSupported,
} from "../../mail/use-read-receipts";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { SettingGroup, SettingRow } from "./section";

const styles = stylex.create({
  trusted: {
    alignItems: "center",
    color: colors.fg3,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    margin: 0,
  },
  warning: { color: colors.warn, display: "block", marginTop: "4px" },
  error: { color: colors.danger, display: "block", marginTop: "4px" },
});

const fullDate = (until: number): string =>
  new Intl.DateTimeFormat(getLocale(), {
    day: "numeric",
    month: "long",
  }).format(new Date(until));

const PreloadSwitch = ({
  failed,
  full,
  onChange,
  optOut,
  saving,
}: {
  readonly failed: boolean;
  /** The quota is full (H15): say so, and until when. */
  readonly full: PreloadFull | null;
  readonly onChange: (checked: boolean) => void;
  readonly optOut: boolean;
  readonly saving: boolean;
}) => (
  <SettingGroup>
    <Switch
      checked={!optOut}
      description={
        <>
          {m.settings_preload_hint()}
          {full === null ? null : (
            <span role="note" {...stylex.props(styles.warning)}>
              {full.until === null
                ? m.settings_preload_full_unknown()
                : m.settings_preload_full({ date: fullDate(full.until) })}
            </span>
          )}
          {failed ? (
            <span role="alert" {...stylex.props(styles.error)}>
              {m.settings_preload_error()}
            </span>
          ) : null}
        </>
      }
      label={m.settings_preload()}
      onCheckedChange={onChange}
      readOnly={saving}
    />
  </SettingGroup>
);

const RECEIPT_LABEL: Readonly<Record<ReadReceiptMode, () => string>> = {
  always: m.settings_read_receipts_always,
  ask: m.settings_read_receipts_ask,
  never: m.settings_read_receipts_never,
};

/** "Read receipts: never / ask / always" (synced). */
const ReadReceiptsControl = () => {
  const [mode, setMode] = useReadReceiptMode();
  return (
    <SettingGroup>
      <SettingRow
        description={m.settings_read_receipts_hint()}
        label={m.settings_read_receipts()}
      >
        <SegmentedControl
          label={m.settings_read_receipts()}
          onValueChange={setMode}
          options={READ_RECEIPT_MODES.map((value) => ({
            label: RECEIPT_LABEL[value](),
            value,
          }))}
          value={mode}
        />
      </SettingRow>
    </SettingGroup>
  );
};

const REMOTE_IMAGES_LABEL: Readonly<Record<RemoteImageMode, () => string>> = {
  blocked: m.settings_remote_images_blocked,
  direct: m.settings_remote_images_direct,
  proxy: m.settings_remote_images_proxy,
};

/** Static build: "Remote images: always blocked / ask" (synced). */
const RemoteImagesControl = () => {
  const [mode, setMode] = useRemoteImageMode();
  return (
    <SettingGroup>
      <SettingRow
        description={
          AVAILABLE_REMOTE_IMAGE_MODES.includes("proxy")
            ? m.settings_remote_images_hint_proxy()
            : m.settings_remote_images_hint()
        }
        label={m.settings_remote_images()}
        wide={AVAILABLE_REMOTE_IMAGE_MODES.length > 3}
      >
        <SegmentedControl
          label={m.settings_remote_images()}
          onValueChange={setMode}
          options={AVAILABLE_REMOTE_IMAGE_MODES.map((value) => ({
            label: REMOTE_IMAGES_LABEL[value](),
            value,
          }))}
          value={mode}
        />
      </SettingRow>
    </SettingGroup>
  );
};

const LOAD_LABEL: Readonly<Record<ImageLoadMode, () => string>> = {
  always: m.settings_images_always,
  ask: m.settings_images_ask,
  never: m.settings_images_never,
  "trusted-senders": m.settings_images_trusted,
};

const ROUTE_LABEL: Readonly<Record<ImageRoute, () => string>> = {
  direct: m.settings_images_via_direct,
  proxy: m.settings_images_via_proxy,
};

/** How they load: proxy / direct, or what the server leaves. */
const ImageRouteControl = ({
  route,
  routeChosen,
  routes,
  setRoute,
}: {
  readonly route: ImageRoute;
  readonly routeChosen: boolean;
  readonly routes: readonly ImageRoute[];
  readonly setRoute: (route: ImageRoute) => void;
}) => {
  if (routes.length > 1) {
    return (
      <SettingGroup>
        <SettingRow
          description={m.settings_images_via_hint()}
          label={m.settings_images_via()}
        >
          <SegmentedControl
            label={m.settings_images_via()}
            onValueChange={setRoute}
            options={routes.map((value) => ({
              label: ROUTE_LABEL[value](),
              value,
            }))}
            value={route}
          />
        </SettingRow>
      </SettingGroup>
    );
  }
  if (route === "proxy") {
    return (
      <SettingGroup>
        <SettingRow
          description={m.settings_images_proxy_only()}
          label={m.settings_images_via()}
        />
      </SettingGroup>
    );
  }
  // No proxy: direct is the only way; loading without a click needs it
  // switched on here (image-policy.ts). "proxy" means "ask first" then.
  return (
    <SettingGroup>
      <Switch
        checked={routeChosen}
        description={m.settings_images_direct_only()}
        label={m.settings_images_direct_auto()}
        onCheckedChange={(checked) => {
          setRoute(checked ? "direct" : "proxy");
        }}
      />
    </SettingGroup>
  );
};

/**
 * Build with our server: "Remote images: never / ask / trusted senders /
 * always" and how they load (synced), as the admin allows.
 */
const ServerImagesControl = () => {
  const settings = useImageSettings();
  const { policy } = settings;
  if (policy === null) {
    return null;
  }
  if (policy.forced || policy.route === null) {
    return (
      <SettingGroup>
        <SettingRow
          description={
            <span role="note">
              {policy.forced
                ? m.settings_images_forced()
                : m.settings_images_none()}
            </span>
          }
          label={m.settings_remote_images()}
        />
      </SettingGroup>
    );
  }
  const trustedCount = settings.trustedSenders.length;
  return (
    <>
      <SettingGroup>
        <SettingRow
          description={m.settings_images_hint()}
          label={m.settings_remote_images()}
          wide={policy.loadModes.length > 3}
        >
          <SegmentedControl
            label={m.settings_remote_images()}
            onValueChange={(mode) => {
              settings.setLoad(mode);
            }}
            options={policy.loadModes.map((value) => ({
              label: LOAD_LABEL[value](),
              value,
            }))}
            value={policy.load}
          />
        </SettingRow>
        {trustedCount > 0 ? (
          <p {...stylex.props(styles.trusted)}>
            {m.settings_images_trusted_list({ count: trustedCount })}
            <Button
              onClick={() => {
                settings.clearTrusted();
              }}
              size="sm"
              variant="ghost"
            >
              {m.settings_images_trusted_clear()}
            </Button>
          </p>
        ) : null}
      </SettingGroup>
      {policy.load === "never" ? null : (
        <ImageRouteControl
          route={policy.route}
          routeChosen={policy.routeChosen}
          routes={policy.routes}
          setRoute={settings.setRoute}
        />
      )}
    </>
  );
};

/** Static build: one-click unsubscribe, off by default (synced). */
const OneClickControl = () => {
  const [enabled, setEnabled] = useOneClickUnsubscribe();
  return (
    <SettingGroup>
      <Switch
        checked={enabled}
        description={m.settings_one_click_hint()}
        label={m.settings_one_click()}
        onCheckedChange={setEnabled}
      />
    </SettingGroup>
  );
};

/**
 * The privacy rows (General puts them under «Privacy»): always there, with the remote images choice (each
 * build its own); read receipts and preload only when the server has them.
 */
export const PreloadControl = () => {
  const receipts = useReceiptsSupported();
  const queryClient = useQueryClient();
  const query = usePreloadStatus();
  const state: PreloadState | { readonly kind: "loading" } = query.data ?? {
    kind: "loading",
  };
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);

  const change = async (checked: boolean): Promise<void> => {
    setSaving(true);
    setFailed(false);
    try {
      queryClient.setQueryData(
        preloadStatusKey,
        await postPreload({ optOut: !checked })
      );
    } catch {
      setFailed(true);
    }
    setSaving(false);
  };

  const staticBuild = import.meta.env.NEBULA_STATIC;
  return (
    <>
      {staticBuild ? <RemoteImagesControl /> : <ServerImagesControl />}
      {staticBuild ? <OneClickControl /> : null}
      {receipts ? <ReadReceiptsControl /> : null}
      {state.kind === "ready" ? (
        <PreloadSwitch
          failed={failed}
          full={state.optOut ? null : state.full}
          onChange={(checked) => {
            void change(checked);
          }}
          optOut={state.optOut}
          saving={saving}
        />
      ) : null}
      {state.kind === "too-many" ? (
        <SettingGroup>
          <SettingRow
            description={m.settings_preload_too_many()}
            label={m.settings_preload()}
          />
        </SettingGroup>
      ) : null}
    </>
  );
};
