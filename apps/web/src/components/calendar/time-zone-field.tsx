// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A time zone picker: "none" plus every IANA zone the browser knows, with
// its current offset. Used by the event editor and the calendar settings.

import { Field, Select } from "@nebulamail/ui";
import type { SelectEntry, SelectOption } from "@nebulamail/ui";

import { useTimeZone } from "../../calendar/hooks";
import { knownTimeZones, timeZoneLabel } from "../../calendar/time";
import { useNow } from "../../lib/use-now";
import { m } from "../../paraglide/messages.js";
import { useDateLocale } from "./use-calendar-prefs";

/** "none" as a select value (a zone id never is empty). */
const NONE = "";

const REGIONS: Readonly<Record<string, () => string>> = {
  Africa: m.cal_tz_region_africa,
  America: m.cal_tz_region_america,
  Antarctica: m.cal_tz_region_antarctica,
  Arctic: m.cal_tz_region_arctic,
  Asia: m.cal_tz_region_asia,
  Atlantic: m.cal_tz_region_atlantic,
  Australia: m.cal_tz_region_australia,
  Europe: m.cal_tz_region_europe,
  Indian: m.cal_tz_region_indian,
  Pacific: m.cal_tz_region_pacific,
};

/**
 * The zones grouped by their IANA region ("Europe/Madrid" under Europe),
 * in the order the browser lists them; zones of no known region (UTC, old
 * aliases such as "US/Eastern") go in a last group.
 */
const zoneEntries = (
  zones: readonly string[],
  label: (zone: string) => string
): SelectEntry<string>[] => {
  const groups = new Map<string, SelectOption<string>[]>();
  const other: SelectOption<string>[] = [];
  for (const zone of zones) {
    const region = zone.split("/")[0] ?? "";
    const option = { label: label(zone), value: zone };
    if (zone.includes("/") && Object.hasOwn(REGIONS, region)) {
      groups.set(region, [...(groups.get(region) ?? []), option]);
    } else {
      other.push(option);
    }
  }
  return [
    ...[...groups].map(([region, options]) => ({
      label: REGIONS[region]?.() ?? region,
      options,
    })),
    ...(other.length === 0
      ? []
      : [{ label: m.cal_tz_region_other(), options: other }]),
  ];
};

export const TimeZoneField = ({
  description,
  label,
  noneLabel,
  onChange,
  value,
}: {
  readonly value: string | null;
  readonly onChange: (zone: string | null) => void;
  readonly label: string;
  /** The text of "no zone" (null). */
  readonly noneLabel: string;
  readonly description?: string;
}) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const now = useNow();
  // The chosen zone and the user's are always there, even if the browser
  // doesn't list them.
  const choices = [
    ...new Set([...knownTimeZones(), zone, ...(value === null ? [] : [value])]),
  ];
  return (
    <Field
      label={label}
      {...(description === undefined ? {} : { description })}
    >
      <Select
        label={label}
        onValueChange={(next) => {
          onChange(next === NONE ? null : next);
        }}
        options={[
          { label: noneLabel, value: NONE },
          ...zoneEntries(choices, (choice) =>
            timeZoneLabel(choice, locale, now)
          ),
        ]}
        value={value ?? NONE}
      />
    </Field>
  );
};
