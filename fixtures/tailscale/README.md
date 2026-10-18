tailscaled's LocalAPI answers (GET /localapi/v0/status and /whois), shaped
as Tailscale 1.104 marshals them (`ipnstate.Status`, `apitype.WhoIsResponse`;
Go writes empty slices and maps as `null`). wad-tailscale's tests read them,
and wadd's tailnet tests serve them from a fake tailscaled. Made up: the
keys, ids and addresses aren't anyone's.
