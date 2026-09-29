import type { NetworkConfig, NetworkConfigUpdate } from '../types';

export function networkConfigUpdate(
  config: NetworkConfig,
  secret: string,
  peerText: string,
): NetworkConfigUpdate {
  const network_name = config.network_name.trim();
  secret = secret.trim();
  const peers = [
    ...new Set(
      peerText
        .split(/\r?\n/)
        .map((peer) => peer.trim())
        .filter(Boolean),
    ),
  ];
  if (
    !network_name ||
    [...network_name].length > 128 ||
    /[\r\n\t]/.test(network_name)
  )
    throw new Error('请填写 1–128 个字符的网络名称。');
  if (secret && !/^[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=$/.test(secret))
    throw new Error('credential 格式无效，请粘贴管理员签发的 Base64 凭据。');
  if (!peers.length || peers.length > 16)
    throw new Error('请填写 1–16 个连接节点，每行一个。');
  for (const peer of peers) {
    let url: URL;
    try {
      url = new URL(peer);
    } catch {
      throw new Error('节点地址无效，例如 tcp://服务器:11010。');
    }
    const websocket = ['ws:', 'wss:'].includes(url.protocol);
    if (
      !['tcp:', 'udp:', 'quic:', 'ws:', 'wss:'].includes(url.protocol) ||
      !url.hostname ||
      (!websocket && !url.port) ||
      url.port === '0' ||
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      (!websocket && url.pathname && url.pathname !== '/')
    ) {
      throw new Error('请填写有效的节点地址，不要包含账号或查询参数。');
    }
  }
  const gateway_ip = config.gateway_ip?.trim() || null;
  if (gateway_ip && !/^(?:\d{1,3}\.){3}\d{1,3}$/.test(gateway_ip))
    throw new Error('校园网关请填写 IPv4 地址。');
  if (gateway_ip && gateway_ip.split('.').some((part) => Number(part) > 255))
    throw new Error('校园网关地址无效。');
  return {
    network_name,
    ...(secret ? { credential_secret: secret } : {}),
    peers,
    private_mode: config.private_mode,
    disable_p2p: config.disable_p2p,
    gateway_ip,
  };
}
