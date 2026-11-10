// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `DAV:` response header (RFC 4918 §10.1).

/// What a server announces in its `DAV:` header (of an OPTIONS answer).
///
/// `sync-collection` is not announced here: it is in each collection's
/// `DAV:supported-report-set` ([`ReportSet`](crate::ReportSet)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent compliance classes of one header"
)]
pub struct DavCapabilities {
    /// Class `1` (basic WebDAV).
    pub class_1: bool,
    /// Class `2` (locking).
    pub class_2: bool,
    /// Class `3` (RFC 4918 revision).
    pub class_3: bool,
    /// `access-control` (RFC 3744).
    pub access_control: bool,
    /// `calendar-access` (CalDAV, RFC 4791).
    pub calendar_access: bool,
    /// `calendar-auto-schedule` (RFC 6638): the server sends invitations
    /// itself.
    pub calendar_auto_schedule: bool,
    /// `calendar-schedule` (older Apple name for scheduling).
    pub calendar_schedule: bool,
    /// `addressbook` (CardDAV, RFC 6352).
    pub addressbook: bool,
    /// `extended-mkcol` (RFC 5689).
    pub extended_mkcol: bool,
    /// `calendarserver-sharing`: calendars are shared with a `CS:share`
    /// POST (Apple, sabre/dav).
    pub calendarserver_sharing: bool,
    /// `resource-sharing` (draft-pot-webdav-resource-sharing; sabre/dav).
    pub resource_sharing: bool,
    /// `oc-resource-sharing`: Nextcloud's own sharing of calendars and
    /// address books (`O:share`). Nextcloud also announces
    /// `calendarserver-sharing` but answers a `CS:share` with 501.
    pub oc_resource_sharing: bool,
    /// `oc-calendar-publishing`: Nextcloud publishes a calendar at a
    /// public link (`CS:publish-calendar`, `CS:publish-url`).
    pub oc_calendar_publishing: bool,
    /// `calendarserver-subscribed`: webcal subscriptions are collections
    /// of the calendar home (`CS:subscribed`, `CS:source`).
    pub calendarserver_subscribed: bool,
    /// `nc-calendar-webcal-cache`: Nextcloud serves the events of a
    /// subscription (its own cached copy) to a request with
    /// `X-NC-CalDAV-Webcal-Caching: On`.
    pub nc_calendar_webcal_cache: bool,
    /// `nc-calendar-trashbin`: deleted calendars and events wait in
    /// `<home>/trashbin/` (Nextcloud).
    pub nc_calendar_trashbin: bool,
}

impl DavCapabilities {
    /// Most tokens read (the rest are ignored).
    pub const MAX_TOKENS: usize = 128;

    /// Reads every `DAV:` header value of a response. Tokens are
    /// comma-separated and case-insensitive; coded URLs (`<http://…>`) and
    /// unknown tokens are ignored.
    ///
    /// ```
    /// use nebula_dav_proto::DavCapabilities;
    ///
    /// let caps = DavCapabilities::from_headers([
    ///     "1, 2, 3, access-control, calendar-access",
    ///     "calendar-auto-schedule, <http://apache.org/dav/propset/fs/1>, addressbook",
    /// ]);
    /// assert!(caps.calendar_access && caps.addressbook && caps.calendar_auto_schedule);
    /// assert!(!caps.extended_mkcol);
    ///
    /// // Baïkal 0.10.1 (sabre/dav 4.7).
    /// let sabre = DavCapabilities::from_headers([
    ///     "1, 3, extended-mkcol, access-control, calendarserver-principal-property-search, calendar-access, calendar-proxy, calendar-auto-schedule, calendar-availability, resource-sharing, calendarserver-sharing, addressbook",
    /// ]);
    /// assert!(sabre.calendarserver_sharing && sabre.resource_sharing && sabre.access_control);
    ///
    /// // Nextcloud 34, on a calendar home.
    /// let nextcloud = DavCapabilities::from_headers([
    ///     "1, 3, extended-mkcol, access-control, calendarserver-principal-property-search, oc-resource-sharing, calendar-access, calendar-proxy, calendar-auto-schedule, calendar-availability, nc-calendar-trashbin, nc-calendar-webcal-cache, calendarserver-subscribed, oc-calendar-publishing, calendarserver-sharing, nc-paginate, nextcloud-checksum-update, nc-calendar-search, nc-enable-birthday-calendar, 2",
    /// ]);
    /// assert!(nextcloud.oc_resource_sharing && !nextcloud.resource_sharing);
    /// assert!(nextcloud.oc_calendar_publishing && nextcloud.calendarserver_subscribed);
    /// assert!(nextcloud.nc_calendar_webcal_cache && nextcloud.nc_calendar_trashbin);
    /// assert!(!sabre.nc_calendar_trashbin && !sabre.oc_calendar_publishing);
    /// ```
    pub fn from_headers<'a>(values: impl IntoIterator<Item = &'a str>) -> Self {
        let mut caps = Self::default();
        let tokens = values
            .into_iter()
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .take(Self::MAX_TOKENS);
        for token in tokens {
            let flag = match token.to_ascii_lowercase().as_str() {
                "1" => &mut caps.class_1,
                "2" => &mut caps.class_2,
                "3" => &mut caps.class_3,
                "access-control" => &mut caps.access_control,
                "calendar-access" => &mut caps.calendar_access,
                "calendar-auto-schedule" => &mut caps.calendar_auto_schedule,
                "calendar-schedule" => &mut caps.calendar_schedule,
                "addressbook" => &mut caps.addressbook,
                "extended-mkcol" => &mut caps.extended_mkcol,
                "calendarserver-sharing" => &mut caps.calendarserver_sharing,
                "resource-sharing" => &mut caps.resource_sharing,
                "oc-resource-sharing" => &mut caps.oc_resource_sharing,
                "oc-calendar-publishing" => &mut caps.oc_calendar_publishing,
                "calendarserver-subscribed" => &mut caps.calendarserver_subscribed,
                "nc-calendar-webcal-cache" => &mut caps.nc_calendar_webcal_cache,
                "nc-calendar-trashbin" => &mut caps.nc_calendar_trashbin,
                _ => continue,
            };
            *flag = true;
        }
        caps
    }

    /// Joins what two answers announced (CalDAV and CardDAV homes may be
    /// different places on one server).
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            class_1: self.class_1 || other.class_1,
            class_2: self.class_2 || other.class_2,
            class_3: self.class_3 || other.class_3,
            access_control: self.access_control || other.access_control,
            calendar_access: self.calendar_access || other.calendar_access,
            calendar_auto_schedule: self.calendar_auto_schedule || other.calendar_auto_schedule,
            calendar_schedule: self.calendar_schedule || other.calendar_schedule,
            addressbook: self.addressbook || other.addressbook,
            extended_mkcol: self.extended_mkcol || other.extended_mkcol,
            calendarserver_sharing: self.calendarserver_sharing || other.calendarserver_sharing,
            resource_sharing: self.resource_sharing || other.resource_sharing,
            oc_resource_sharing: self.oc_resource_sharing || other.oc_resource_sharing,
            oc_calendar_publishing: self.oc_calendar_publishing || other.oc_calendar_publishing,
            calendarserver_subscribed: self.calendarserver_subscribed
                || other.calendarserver_subscribed,
            nc_calendar_webcal_cache: self.nc_calendar_webcal_cache
                || other.nc_calendar_webcal_cache,
            nc_calendar_trashbin: self.nc_calendar_trashbin || other.nc_calendar_trashbin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DavCapabilities;

    #[test]
    fn case_and_spacing() {
        let caps = DavCapabilities::from_headers(["  CALENDAR-ACCESS ,,Addressbook  "]);
        assert!(caps.calendar_access && caps.addressbook);
        assert!(!caps.class_1);
    }

    #[test]
    fn token_limit() {
        let many = format!(
            "{}calendar-access",
            "x,".repeat(DavCapabilities::MAX_TOKENS)
        );
        assert!(!DavCapabilities::from_headers([many.as_str()]).calendar_access);
    }
}
