// Mock MCP Server Dashboard — Client-side JS

(function () {
  'use strict';

  let ws = null;
  let tools = [];
  let clients = new Map();

  // --- DOM refs ---
  const statusEl = document.getElementById('connection-status');
  const statusLabel = statusEl.querySelector('.label');
  const delaySlider = document.getElementById('delay-slider');
  const delayValue = document.getElementById('delay-value');
  const failRateSlider = document.getElementById('fail-rate-slider');
  const failRateValue = document.getElementById('fail-rate-value');
  const pauseToggle = document.getElementById('pause-toggle');
  const toolOverrides = document.getElementById('tool-overrides');
  const clientsList = document.getElementById('clients-list');
  const logBody = document.getElementById('log-body');
  const logTable = document.getElementById('log-table');
  const logEmpty = document.getElementById('log-empty');
  const clearLogBtn = document.getElementById('clear-log');

  // --- WebSocket connection ---
  function connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = () => {
      statusEl.className = 'status connected';
      statusLabel.textContent = 'Connected';
    };

    ws.onclose = () => {
      statusEl.className = 'status disconnected';
      statusLabel.textContent = 'Disconnected';
      setTimeout(connect, 2000);
    };

    ws.onerror = () => {
      ws.close();
    };

    ws.onmessage = (event) => {
      const msg = JSON.parse(event.data);
      handleMessage(msg);
    };
  }

  function send(msg) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }

  // --- Message handling ---
  function handleMessage(msg) {
    switch (msg.type) {
      case 'init':
        tools = msg.data.tools;
        renderToolOverrides(msg.data.settings.overrides || {});
        applySettings(msg.data.settings);
        renderLog(msg.data.log);
        if (msg.data.clients) {
          for (const c of msg.data.clients) {
            clients.set(c.clientId, c);
          }
          renderClients();
        }
        break;
      case 'request':
        addLogRow(msg.data, null);
        break;
      case 'response':
        updateLogRow(msg.data);
        break;
      case 'connection':
        updateClients(msg.data);
        break;
      case 'settings_changed':
        applySettings(msg.data);
        break;
    }
  }

  // --- Tool Overrides ---
  function renderToolOverrides(overrides) {
    toolOverrides.innerHTML = '';
    for (const tool of tools) {
      const card = document.createElement('div');
      card.className = 'tool-card';

      const h3 = document.createElement('h3');
      h3.textContent = tool.name;

      const desc = document.createElement('p');
      desc.textContent = tool.description;

      const select = document.createElement('select');
      select.dataset.tool = tool.name;

      const defaultOpt = document.createElement('option');
      defaultOpt.value = '';
      defaultOpt.textContent = `Default (${tool.defaultResponseKey})`;
      select.appendChild(defaultOpt);

      for (const resp of tool.responses) {
        const opt = document.createElement('option');
        opt.value = resp.key;
        opt.textContent = `${resp.label}${resp.isError ? ' ❌' : ' ✓'}`;
        if (overrides[tool.name] === resp.key) {
          opt.selected = true;
        }
        select.appendChild(opt);
      }

      select.addEventListener('change', () => {
        send({
          type: 'set_override',
          toolName: tool.name,
          responseKey: select.value || null,
        });
      });

      card.appendChild(h3);
      card.appendChild(desc);
      card.appendChild(select);
      toolOverrides.appendChild(card);
    }
  }

  // --- Settings ---
  function applySettings(settings) {
    delaySlider.value = settings.delay;
    delayValue.textContent = settings.delay + ' ms';
    failRateSlider.value = Math.round(settings.failRate * 100);
    failRateValue.textContent = Math.round(settings.failRate * 100) + '%';

    if (settings.paused) {
      pauseToggle.textContent = '⏸ Paused';
      pauseToggle.classList.add('paused');
    } else {
      pauseToggle.textContent = '▶ Running';
      pauseToggle.classList.remove('paused');
    }
  }

  delaySlider.addEventListener('input', () => {
    const v = parseInt(delaySlider.value, 10);
    delayValue.textContent = v + ' ms';
  });

  delaySlider.addEventListener('change', () => {
    send({ type: 'set_settings', data: { delay: parseInt(delaySlider.value, 10) } });
  });

  failRateSlider.addEventListener('input', () => {
    const v = parseInt(failRateSlider.value, 10);
    failRateValue.textContent = v + '%';
  });

  failRateSlider.addEventListener('change', () => {
    send({ type: 'set_settings', data: { failRate: parseInt(failRateSlider.value, 10) / 100 } });
  });

  pauseToggle.addEventListener('click', () => {
    const isPaused = pauseToggle.classList.contains('paused');
    send({ type: 'set_settings', data: { paused: !isPaused } });
  });

  clearLogBtn.addEventListener('click', () => {
    logBody.innerHTML = '';
    logTable.classList.remove('has-rows');
    logEmpty.classList.remove('hidden');
    send({ type: 'clear_log' });
  });

  // --- Clients ---
  function updateClients(event) {
    if (event.connected) {
      clients.set(event.clientId, event);
    } else {
      clients.delete(event.clientId);
    }
    renderClients();
  }

  function renderClients() {
    if (clients.size === 0) {
      clientsList.innerHTML = '<p class="empty-state">No clients connected</p>';
      return;
    }
    clientsList.innerHTML = '';
    for (const [id, info] of clients) {
      const badge = document.createElement('span');
      badge.className = 'client-badge';
      badge.innerHTML = `<span class="dot"></span>${info.transport}:${id.slice(0, 8)}`;
      clientsList.appendChild(badge);
    }
  }

  // --- Request Log ---
  function renderLog(log) {
    logBody.innerHTML = '';
    for (const entry of log) {
      addLogRow(entry.request, entry.response || null);
    }
  }

  function addLogRow(request, response) {
    logTable.classList.add('has-rows');
    logEmpty.classList.add('hidden');

    const tr = document.createElement('tr');
    tr.dataset.requestId = request.id;
    tr.className = 'new-row';
    setTimeout(() => tr.classList.remove('new-row'), 1100);

    const time = new Date(request.timestamp);
    const timeStr = time.toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
      + '.' + String(time.getMilliseconds()).padStart(3, '0');

    tr.innerHTML = `
      <td class="time">${timeStr}</td>
      <td class="tool-name">${escapeHtml(request.toolName)}</td>
      <td class="args" title="${escapeHtml(JSON.stringify(request.arguments))}">${escapeHtml(JSON.stringify(request.arguments))}</td>
      <td>${response ? statusBadge(response.isError) : '<span class="badge pending">pending</span>'}</td>
      <td class="duration">${response ? response.duration + 'ms' : '—'}</td>
      <td class="response-text">${response ? escapeHtml(responseText(response.content)) : '—'}</td>
    `;

    logBody.prepend(tr);

    // Keep log to 500 entries
    while (logBody.children.length > 500) {
      logBody.removeChild(logBody.lastChild);
    }
  }

  function updateLogRow(response) {
    const tr = logBody.querySelector(`tr[data-request-id="${response.requestId}"]`);
    if (!tr) return;

    const cells = tr.querySelectorAll('td');
    cells[3].innerHTML = statusBadge(response.isError);
    cells[4].textContent = response.duration + 'ms';
    cells[5].className = 'response-text';
    cells[5].textContent = responseText(response.content);
    cells[5].title = responseText(response.content);
  }

  function statusBadge(isError) {
    return isError
      ? '<span class="badge error">error</span>'
      : '<span class="badge success">ok</span>';
  }

  function responseText(content) {
    if (!content || content.length === 0) return '(empty)';
    return content.map(c => c.text || '').join('\n');
  }

  function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
  }

  // --- Init ---
  connect();

  // --- Sandbox Probes ---
  (function initProbes() {
    const tabs = document.querySelectorAll('.probe-tab');
    const contents = document.querySelectorAll('.probe-content');
    const fsOperation = document.getElementById('fs-operation');
    const fsPath = document.getElementById('fs-path');
    const fsContent = document.getElementById('fs-content');
    const fsContentRow = document.getElementById('fs-content-row');
    const fsExecute = document.getElementById('fs-execute');
    const fsResults = document.getElementById('fs-results');
    const netMethod = document.getElementById('net-method');
    const netUrl = document.getElementById('net-url');
    const netHeaders = document.getElementById('net-headers');
    const netBody = document.getElementById('net-body');
    const netExecute = document.getElementById('net-execute');
    const netResults = document.getElementById('net-results');

    // Tab switching
    tabs.forEach(tab => {
      tab.addEventListener('click', () => {
        tabs.forEach(t => t.classList.remove('active'));
        contents.forEach(c => c.classList.remove('active'));
        tab.classList.add('active');
        const target = document.getElementById('probe-tab-' + tab.dataset.tab);
        if (target) target.classList.add('active');
      });
    });

    // Show/hide write content area
    fsOperation.addEventListener('change', () => {
      fsContentRow.style.display = fsOperation.value === 'write' ? '' : 'none';
    });

    // File system probe
    fsExecute.addEventListener('click', async () => {
      const op = fsOperation.value;
      const pathVal = fsPath.value.trim();
      if (!pathVal) { fsPath.focus(); return; }

      fsExecute.disabled = true;
      fsExecute.textContent = 'Running…';

      let endpoint, body;
      if (op === 'read') {
        endpoint = '/api/probe/file-read';
        body = { path: pathVal };
      } else if (op === 'write') {
        endpoint = '/api/probe/file-write';
        body = { path: pathVal, content: fsContent.value };
      } else {
        endpoint = '/api/probe/dir-list';
        body = { path: pathVal };
      }

      try {
        const res = await fetch(endpoint, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        const result = await res.json();
        renderFsResult(op, pathVal, result);
      } catch (err) {
        renderFsResult(op, pathVal, { success: false, duration: 0, error: err.message });
      } finally {
        fsExecute.disabled = false;
        fsExecute.textContent = 'Execute';
      }
    });

    // Network probe
    netExecute.addEventListener('click', async () => {
      const url = netUrl.value.trim();
      if (!url) { netUrl.focus(); return; }

      netExecute.disabled = true;
      netExecute.textContent = 'Running…';

      const method = netMethod.value;
      const headersText = netHeaders.value.trim();
      const bodyText = netBody.value.trim();

      const headers = {};
      if (headersText) {
        for (const line of headersText.split('\n')) {
          const idx = line.indexOf(':');
          if (idx > 0) {
            headers[line.slice(0, idx).trim()] = line.slice(idx + 1).trim();
          }
        }
      }

      const payload = { url, method };
      if (bodyText && method !== 'GET' && method !== 'HEAD') {
        payload.body = bodyText;
      }
      if (Object.keys(headers).length > 0) {
        payload.headers = headers;
      }

      try {
        const res = await fetch('/api/probe/network', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
        });
        const result = await res.json();
        renderNetResult(method, url, result);
      } catch (err) {
        renderNetResult(method, url, { success: false, duration: 0, error: err.message });
      } finally {
        netExecute.disabled = false;
        netExecute.textContent = 'Execute';
      }
    });

    function clearEmpty(container) {
      const empty = container.querySelector('.empty-state');
      if (empty) empty.remove();
    }

    function renderFsResult(op, pathVal, result) {
      clearEmpty(fsResults);

      const entry = document.createElement('div');
      entry.className = 'probe-result-entry';

      const opLabel = { read: 'READ', write: 'WRITE', list: 'LIST' }[op] || op.toUpperCase();
      const badge = result.success
        ? '<span class="badge success">ok</span>'
        : '<span class="badge error">error</span>';

      let bodyContent = '';
      if (result.success && result.data) {
        if (op === 'list') {
          const d = result.data;
          const lines = d.entries.map(e => {
            const sizeStr = e.size !== undefined ? `  (${formatBytes(e.size)})` : '';
            return `${e.type === 'directory' ? '📁' : '📄'} ${e.name}${sizeStr}`;
          }).join('\n');
          bodyContent = `${d.count} entries${d.truncated ? ' (truncated to 200)' : ''}:\n\n${lines}`;
        } else if (op === 'read') {
          const d = result.data;
          bodyContent = `Size: ${formatBytes(d.size)}${d.truncated ? ' (truncated to 64KB)' : ''}\n\n${d.content}`;
        } else {
          bodyContent = JSON.stringify(result.data, null, 2);
        }
      } else if (result.error) {
        bodyContent = result.error;
      }

      const needsCollapse = bodyContent.split('\n').length > 4 || bodyContent.length > 300;

      entry.innerHTML = `
        <div class="probe-result-header">
          ${badge}
          <span class="probe-result-label">${opLabel} ${escapeHtml(pathVal)}</span>
          <span class="probe-result-duration">${result.duration}ms</span>
        </div>
        <div class="probe-result-body ${needsCollapse ? 'collapsed' : ''}">${escapeHtml(bodyContent)}</div>
      `;

      const bodyEl = entry.querySelector('.probe-result-body');
      if (needsCollapse) {
        bodyEl.addEventListener('click', () => {
          bodyEl.classList.toggle('collapsed');
          bodyEl.classList.toggle('expanded');
        });
      }

      fsResults.prepend(entry);
      trimResults(fsResults);
    }

    function renderNetResult(method, url, result) {
      clearEmpty(netResults);

      const entry = document.createElement('div');
      entry.className = 'probe-result-entry';

      const badge = result.success
        ? '<span class="badge success">ok</span>'
        : '<span class="badge error">error</span>';

      let metaHtml = '';
      let bodyContent = '';

      if (result.success && result.data) {
        const d = result.data;
        metaHtml = `
          <div class="probe-result-meta">
            <span><span class="meta-key">Status:</span> ${d.status} ${escapeHtml(d.statusText)}</span>
            <span><span class="meta-key">Size:</span> ${formatBytes(d.bodyLength)}</span>
            ${d.truncated ? '<span>(truncated to 64KB)</span>' : ''}
          </div>
        `;
        bodyContent = d.body || '(empty body)';
      } else if (result.error) {
        bodyContent = result.error;
      }

      const needsCollapse = bodyContent.split('\n').length > 4 || bodyContent.length > 300;

      entry.innerHTML = `
        <div class="probe-result-header">
          ${badge}
          <span class="probe-result-label">${escapeHtml(method)} ${escapeHtml(url)}</span>
          <span class="probe-result-duration">${result.duration}ms</span>
        </div>
        ${metaHtml}
        <div class="probe-result-body ${needsCollapse ? 'collapsed' : ''}">${escapeHtml(bodyContent)}</div>
      `;

      const bodyEl = entry.querySelector('.probe-result-body');
      if (needsCollapse) {
        bodyEl.addEventListener('click', () => {
          bodyEl.classList.toggle('collapsed');
          bodyEl.classList.toggle('expanded');
        });
      }

      netResults.prepend(entry);
      trimResults(netResults);
    }

    function trimResults(container) {
      while (container.children.length > 50) {
        container.removeChild(container.lastChild);
      }
    }

    function formatBytes(bytes) {
      if (bytes < 1024) return bytes + ' B';
      if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
      return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    }
  })();
})();
