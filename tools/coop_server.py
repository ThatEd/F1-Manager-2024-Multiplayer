"""Coop command server for F1 Manager 24.

The game's own Continue button is nothing but `engine.trigger('ContinueGame')` from the
front-end JS (project/modules/frontend/components/MainMenu.js). Our pak override injects
`js/coop/bridge.js` into Frontend.html, and that bridge polls this server. Sending a command
here therefore fires *the same call the button fires*: no input injection, no address hunting,
no debugger that can kill the process when it is force-closed.

Delivery model
--------------
A command is broadcast: it is handed to every client that has not yet received it. Each client
gets it once, in order. That is what "everyone press Continue at the same time" needs, and it
degrades sanely if one client is still loading: it receives the command when it next polls.

Endpoints
---------
GET /hello?client=&stage=&ready=&sent=&pkg=&v=   heartbeat; body is the next command for this client
GET /ack?line=&ok=&detail=                      bridge confirms a command ran
GET /pak-ok?file=&bridge=                       first contact; proves the pak override loaded
GET /lobby.js, GET /lobby.rev                   live multiplayer UI source + its revision
GET /cmd?text=Continue[&ttl=60]                 operator: queue a broadcast command
GET /status                                     JSON snapshot of clients + queue
GET /quit                                       stop the server

Nothing here writes to the game. Restart the game after adding or removing the pak.
"""

import json
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = 45999
LOCK = threading.Lock()

clients = {}    # client id -> heartbeat state
queue = []      # list of {'id','text','created','ttl','delivered': set()}
history = []    # every event we observed, kept for evidence
next_id = [1]


def iso():
    return time.strftime('%H:%M:%S')


def note(kind, **kw):
    rec = {'t': iso(), 'kind': kind}
    rec.update(kw)
    history.append(rec)
    del history[:-200]
    print('[%s] %-8s %s' % (rec['t'], kind, json.dumps(kw, sort_keys=True)), flush=True)


def take_for(client_id):
    """Next command this client may actually run, or None.

    Delivery is filtered, not just broadcast. A client that is inside a career cannot safely run
    LoadGame: doing so tears its front-end down without rebuilding it and leaves a black screen with
    the process still alive (observed on a live client). The bridge refuses too, but the server has to
    refuse first, because a stale pak on one machine is exactly how that black screen happened.
    """
    now = time.time()
    for job in list(queue):
        if now - job['created'] > job['ttl']:
            queue.remove(job)
            note('expire', id=job['id'], cmd=job['text'])
            continue
        if client_id in job['delivered']:
            continue
        if client_id in job.get('skipped', ()):
            continue

        page = clients.get(client_id, {}).get('page', '')
        verb = job['text'].strip().split()[0].lower() if job['text'].strip() else ''
        if verb in ('load', 'loadgame', 'continue', 'continuegame') and page != 'Frontend':
            job.setdefault('skipped', set()).add(client_id)
            note('skip', id=job['id'], client=client_id, cmd=job['text'], page=page,
                 why='load/continue need the front-end')
            continue

        job['delivered'].add(client_id)
        return job
    return None


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, fmt, *args):
        pass  # clients poll 4x/s; default logging would drown the console

    def _send(self, code, body, ctype='text/plain; charset=utf-8'):
        raw = body.encode('utf-8')
        self.send_response(code)
        self.send_header('Content-Type', ctype)
        self.send_header('Content-Length', str(len(raw)))
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Access-Control-Allow-Origin', '*')
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(u.query, keep_blank_values=True)

        def one(k, default=''):
            v = q.get(k)
            return v[0] if v else default

        try:
            if u.path == '/bridge.js':
                # Live bridge source. The pak copy is only the bootstrap; once loaded, the bridge
                # polls /bridge.rev and re-evaluates whatever this returns. That is what makes
                # editing bridge logic cost a file write instead of two game restarts.
                import os
                src = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'pak',
                                   'f1manager24', 'Content', 'UIGameface', 'js', 'coop',
                                   'bridge.js')
                try:
                    with open(src, 'r', encoding='utf-8') as f:
                        body = f.read()
                    self._send(200, body, 'application/javascript; charset=utf-8')
                except OSError as exc:
                    self._send(500, 'cannot read bridge: %s' % exc)
                return

            if u.path == '/bridge.rev':
                # The revision is just the source's mtime+size, so saving the file is the whole
                # publish step. No build, no upload, no restart.
                import os
                src = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'pak',
                                   'f1manager24', 'Content', 'UIGameface', 'js', 'coop',
                                   'bridge.js')
                try:
                    st = os.stat(src)
                    self._send(200, '%d-%d' % (int(st.st_mtime), st.st_size))
                except OSError:
                    self._send(200, 'missing')
                return

            # The multiplayer UI lives in lobby.js. Without this, every UI tweak costs a repak and a
            # game restart - and a pak that was never copied looks exactly like code that does not
            # work. Same contract as the bridge: save the file and the page picks it up.
            if u.path in ('/lobby.js', '/lobby.rev'):
                import os
                src = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'pak',
                                   'f1manager24', 'Content', 'UIGameface', 'js', 'coop',
                                   'lobby.js')
                if u.path == '/lobby.rev':
                    try:
                        st = os.stat(src)
                        self._send(200, '%d-%d' % (int(st.st_mtime), st.st_size))
                    except OSError:
                        self._send(200, 'missing')
                    return
                try:
                    with open(src, 'r', encoding='utf-8') as f:
                        body = f.read()
                    self._send(200, body, 'application/javascript; charset=utf-8')
                except OSError as exc:
                    self._send(500, 'cannot read lobby: %s' % exc)
                return

            if u.path == '/hello':
                # The bridge folds the previous command's result into this poll: an in-game Cohtml
                # view resets its network stack on a scene change, so a separate fire-and-forget
                # ack would be lost.
                # The peer address, not a self-reported name: every bridge used to send the
                # *server's* address, so two machines collided into one registry entry and one of
                # them silently stopped being told anything. The socket already knows who it is.
                cid = self.client_address[0]
                ackline = one('ack')
                with LOCK:
                    c = clients.setdefault(cid, {'first': iso(), 'acks': 0})
                    if c.get('stage') != one('stage') or c.get('page') != one('page'):
                        note('stage', client=cid, page=one('page'),
                             stage=one('stage'), sent=one('sent'))
                    c.update(page=one('page'), stage=one('stage'), sent=one('sent'),
                             pkg=one('pkg'), v=one('v'), last=iso())
                    if ackline:
                        ok = ackline.startswith('OK ')
                        c['acks'] = c.get('acks', 0) + 1
                        if ok:
                            c['lastok'] = ackline
                        else:
                            c['lasterr'] = ackline
                        note('ack', client=cid, page=one('page'),
                             ok='1' if ok else '0', text=ackline)
                    job = take_for(cid)
                if job:
                    note('deliver', id=job['id'], client=cid, cmd=job['text'])
                    self._send(200, job['text'])
                else:
                    self._send(200, '')

            elif u.path == '/ack':
                cid = self.client_address[0]
                with LOCK:
                    if cid in clients:
                        clients[cid]['acks'] = clients[cid].get('acks', 0) + 1
                note('ack', client=cid, cmd=one('line'), ok=one('ok'), detail=one('detail'))
                self._send(200, 'ok')

            elif u.path == '/report':
                # Sent synchronously by the bridge immediately before it fires an event that will
                # destroy its own document, so this is the only reliable evidence a command ran.
                text = one('text')
                cid = self.client_address[0]
                with LOCK:
                    if cid in clients:
                        clients[cid]['acks'] = clients[cid].get('acks', 0) + 1
                        if text.startswith('OK '):
                            clients[cid]['lastok'] = text
                        else:
                            clients[cid]['lasterr'] = text
                note('report', client=cid, page=one('page'), text=text)
                self._send(200, 'ok')

            elif u.path == '/pak-ok':
                note('pak-ok', file=one('file'), bridge=one('bridge'))
                self._send(200, 'ok')

            elif u.path == '/cmd':
                text = one('text')
                if not text:
                    self._send(400, 'text= required')
                    return
                try:
                    ttl = float(one('ttl', '60'))
                except ValueError:
                    ttl = 60.0
                with LOCK:
                    job = {'id': next_id[0], 'text': text, 'created': time.time(),
                           'ttl': ttl, 'delivered': set()}
                    next_id[0] += 1
                    queue.append(job)
                    depth = len(queue)
                    n = len(clients)
                note('queue', id=job['id'], cmd=text, ttl=ttl, depth=depth, clients=n)
                self._send(200, 'queued %d: %s (depth %d, %d client(s))'
                           % (job['id'], text, depth, n))

            elif u.path == '/status':
                with LOCK:
                    body = json.dumps({
                        'clients': clients,
                        'queue': [{'id': j['id'], 'text': j['text'],
                                   'delivered': sorted(j['delivered']),
                                   'skipped': sorted(j.get('skipped', ())),
                                   'age': round(time.time() - j['created'], 1),
                                   'ttl': j['ttl']} for j in queue],
                        'history': history[-50:],
                    }, indent=2)
                self._send(200, body, 'application/json')

            elif u.path == '/quit':
                self._send(200, 'bye')
                threading.Thread(target=self.server.shutdown, daemon=True).start()

            else:
                self._send(404, 'unknown endpoint')
        except BrokenPipeError:
            pass
        except Exception as exc:  # keep the server alive: the game polls it
            note('error', path=u.path, err=str(exc))
            try:
                self._send(500, 'server error: %s' % exc)
            except Exception:
                pass


def main():
    # Bind to every interface. The host's own game polls 127.0.0.1 and the other machine polls the
    # host's LAN address; one listener answers both, which is what lets a single bridge pak work on
    # either side. Loopback-only was a bug: the remote client found nothing listening.
    srv = ThreadingHTTPServer(('0.0.0.0', PORT), Handler)
    srv.daemon_threads = True
    print('coop command server on port %d (all interfaces)' % PORT)
    print('  press Continue on every client : curl "http://127.0.0.1:%d/cmd?text=Continue"' % PORT)
    print('  load a save everywhere         : curl "http://127.0.0.1:%d/cmd?text=Load%%20save2"' % PORT)
    print('  status                         : curl "http://127.0.0.1:%d/status"' % PORT)
    print('  stop                           : curl "http://127.0.0.1:%d/quit"' % PORT)
    print('  remote clients reach this at   : http://192.168.1.101:%d/' % PORT)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print('\nstopped')


if __name__ == '__main__':
    main()
