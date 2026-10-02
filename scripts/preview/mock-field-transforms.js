/* Preview-only local codec fixture. Production uses the bounded Rust implementation. */
(() => {
  const encode = value => new TextEncoder().encode(value), decode = value => new TextDecoder('utf-8', { fatal: true }).decode(value);
  const text = value => typeof value === 'string' ? value : JSON.stringify(value);
  function bounded(value, bytes = 512 * 1024) {
    let nodes = 0;
    const visit = (item, depth) => { if (++nodes > 4096 || depth > 16) throw Error('A estrutura excede o limite de profundidade ou campos.'); if (item && typeof item === 'object') Object.values(item).forEach(child => visit(child, depth + 1)); };
    visit(value, 0); if (encode(text(value)).length > bytes) throw Error('O valor excede o limite da transformação.');
    return value;
  }
  const percent = (value, form) => decodeURIComponent(form ? value.replaceAll('+', ' ') : value);
  function base64(value, url) {
    if (!(url ? /^[A-Za-z0-9_-]*={0,2}$/ : /^[A-Za-z0-9+/]*={0,2}$/).test(value) || value.length % 4 === 1) throw Error('Base64 inválido.');
    return decode(Uint8Array.from(atob(url ? value.replaceAll('-', '+').replaceAll('_', '/') : value), char => char.charCodeAt(0)));
  }
  const repeated = (target, key, value) => { if (Object.hasOwn(target, key)) target[key] = Array.isArray(target[key]) ? [...target[key], value] : [target[key], value]; else Object.defineProperty(target, key, { value, enumerable: true, writable: true, configurable: true }); };
  function xml(value) {
    if (/<!DOCTYPE|<!ENTITY/i.test(value)) throw Error('XML inválido.');
    const document = new DOMParser().parseFromString(value, 'application/xml');
    if (document.querySelector('parsererror') || !document.documentElement) throw Error('XML inválido.');
    const name = node => node.namespaceURI ? `{${node.namespaceURI}}${node.localName}` : node.localName;
    function project(node, depth = 0) {
      if (depth > 16) throw Error('XML profundo demais.');
      const out = Object.create(null); let ownText = '';
      for (const attribute of node.attributes) if (attribute.prefix !== 'xmlns' && attribute.name !== 'xmlns') repeated(out, `@${name(attribute)}`, attribute.value);
      for (const child of node.childNodes) {
        if (child.nodeType === 1) repeated(out, name(child), project(child, depth + 1));
        else if (child.nodeType === 3 || child.nodeType === 4) ownText += child.nodeValue;
      }
      if (!Object.keys(out).length) return ownText;
      if (ownText) repeated(out, '#text', ownText); return out;
    }
    return { [name(document.documentElement)]: project(document.documentElement) };
  }
  function transform(value, steps) {
    if (!Array.isArray(steps) || steps.length > 8) throw Error('Use no máximo 8 transformações.');
    bounded(value, 256 * 1024); value = structuredClone(value); const notices = [];
    for (const step of steps) {
      const input = text(value);
      switch (step) {
        case 'base64_decode': case 'base64_url_decode': value = base64(input, step === 'base64_url_decode'); break;
        case 'base64_encode': case 'base64_url_encode': {
          let binary = ''; for (const byte of encode(input)) binary += String.fromCharCode(byte);
          value = btoa(binary); if (step === 'base64_url_encode') value = value.replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, ''); break;
        }
        case 'url_decode': value = percent(input, false); break;
        case 'form_decode': value = percent(input, true); break;
        case 'url_encode': value = encodeURIComponent(input).replace(/[!'()*]/g, char => `%${char.charCodeAt(0).toString(16).toUpperCase()}`); break;
        case 'hex_to_text': if (!/^(?:[0-9a-fA-F]{2})*$/.test(input)) throw Error('Hexadecimal inválido.'); value = decode(Uint8Array.from(input.match(/../g) || [], part => parseInt(part, 16))); break;
        case 'text_to_hex': value = [...encode(input)].map(byte => byte.toString(16).padStart(2, '0')).join(''); break;
        case 'parse_json': value = JSON.parse(input); break;
        case 'parse_xml': value = xml(input); break;
        case 'parse_query': {
          value = Object.create(null); const query = input.replace(/^\?/, ''), pairs = query.split('&');
          for (const pair of query ? pairs : []) { const at = pair.indexOf('='), key = at < 0 ? pair : pair.slice(0, at), part = at < 0 ? '' : pair.slice(at + 1); repeated(value, percent(key, true), percent(part, true)); }
          break;
        }
        case 'jwt_payload': {
          const parts = input.split('.'); if (parts.length !== 3 || !parts[0] || !parts[1] || !/^[A-Za-z0-9_=-]*$/.test(parts[2])) throw Error('JWT inválido.');
          const header = JSON.parse(base64(parts[0], true)); value = JSON.parse(base64(parts[1], true));
          if (!header || Array.isArray(header) || typeof header !== 'object' || !value || Array.isArray(value) || typeof value !== 'object') throw Error('JWT inválido.');
          bounded(header); if (!notices.includes('jwt_signature_not_verified')) notices.push('jwt_signature_not_verified'); break;
        }
        default: throw Error('Transformação desconhecida.');
      }
      bounded(value);
    }
    return { value, notices };
  }
  function expand(name, value) {
    const fields = Object.create(null);
    function visit(path, item) {
      if (Object.keys(fields).length >= 512) throw Error('Campos demais na transformação.');
      if (item && typeof item === 'object') {
        const children = Object.entries(item);
        for (const [key, child] of children.filter(([, value]) => value && typeof value === 'object')) visit(`${path}.${key}`, child);
        for (const [key, child] of children.filter(([, value]) => !value || typeof value !== 'object')) visit(`${path}.${key}`, child);
      }
      Object.defineProperty(fields, path, { value: item, enumerable: true, writable: true, configurable: true });
    }
    visit(name, value); return fields;
  }
  window.__mockFieldTransforms = { transform, expand };
})();
