export function alertRoute(search: string): { page: string; alert?: string; invalid: boolean } {
  const parameters = new URLSearchParams(search);
  const alert = parameters.get('alert');
  if (alert !== null) {
    const valid = /^[a-zA-Z0-9._:-]{1,512}$/.test(alert);
    return { page: 'alerts', alert: valid ? alert : undefined, invalid: !valid };
  }
  return { page: parameters.get('view') === 'alerts' ? 'alerts' : 'overview', invalid: false };
}

export function alertLocation(location: Pick<Location, 'pathname' | 'search' | 'hash'>, id?: string, alerts = false): string {
  const parameters = new URLSearchParams(location.search);
  parameters.delete('alert'); parameters.delete('view');
  if (id) parameters.set('alert', id);
  else if (alerts) parameters.set('view', 'alerts');
  const search = parameters.toString();
  return location.pathname + (search ? `?${search}` : '') + location.hash;
}
