# TEST ONLY. Owned-loopback stand-in for an OpenAI-compatible provider
# (llama-server, Ollama, LM Studio, whisper.cpp). Prints its port, appends one
# JSON line per request to argv[1], and exits after 30 idle seconds.
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = sys.argv[1]


def multipart(content_type, body):
    boundary = content_type.split('boundary=', 1)[1].encode()
    fields, files = {}, {}
    for part in body.split(b'--' + boundary)[1:]:
        if part.startswith(b'--'):
            break
        head, _, value = part[2:].partition(b'\r\n\r\n')
        value = value[:-2]
        disposition = head.split(b'\r\n')[0].decode()
        name = disposition.split('name="', 1)[1].split('"', 1)[0]
        if 'filename="' in disposition:
            files[name] = {'filename': disposition.split('filename="', 1)[1].split('"', 1)[0],
                           'bytes': len(value), 'riff': value[:4] == b'RIFF'}
        else:
            fields.setdefault(name, []).append(value.decode())
    return fields, files


class Handler(BaseHTTPRequestHandler):
    half_bad = 0

    def log_message(self, *args):
        pass

    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def record(self, entry):
        entry['headers'] = {k.lower(): v for k, v in self.headers.items()}
        entry['method'] = self.command
        entry['path'] = self.path
        with open(LOG, 'a') as log:
            log.write(json.dumps(entry) + '\n')

    def do_GET(self):
        self.record({})
        if self.path == '/v1/models':
            self.reply(200, {'object': 'list', 'data': [
                {'id': 'vision-small', 'object': 'model', 'owned_by': 'llamacpp'},
                {'id': 'vlm', 'architecture': {'input_modalities': ['text', 'image'], 'output_modalities': ['text']}},
                {'id': 'text-only', 'architecture': {'input_modalities': ['text'], 'output_modalities': ['text']}},
            ]})
        else:
            self.reply(404, {'error': {'message': 'not found'}})

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        content_type = self.headers.get('Content-Type', '')
        if self.path == '/v1/audio/transcriptions':
            fields, files = multipart(content_type, body)
            self.record({'fields': fields, 'files': files, 'content_type': content_type})
            model = fields.get('model', [''])[0]
            if model == 'text-only-asr':
                self.reply(200, {'text': 'hello there'})
                return
            self.reply(200, {'text': 'hello there', 'words': [
                {'word': 'hello', 'start': 0.2, 'end': 0.5},
                {'word': 'there', 'start': 0.6, 'end': 0.9}]})
            return
        request = json.loads(body)
        self.record({'body': request})
        model = request.get('model')
        images = [part for message in request.get('messages', [])
                  for part in (message.get('content') if isinstance(message.get('content'), list) else [])
                  if part.get('type') == 'image_url']
        if model == 'slow':
            time.sleep(4)
        if model == 'text-only' and images:
            self.reply(500, {'error': {'code': 500, 'type': 'server_error',
                                       'message': 'image input is not supported - hint: if this is unexpected, you may need to provide the mmproj'}})
            return
        content = '<think>\nchecking the frame\n</think>\n{"summary": "a test frame", "people_count": 0}'
        usage = {'prompt_tokens': 10, 'completion_tokens': 5, 'total_tokens': 15}
        if model == 'last-answer':
            usage['cost'] = 0.25
        if model == 'half-bad':
            # The first answer is not JSON; both completed requests report a cost.
            Handler.half_bad += 1
            usage['cost'] = 0.25
            if Handler.half_bad == 1:
                content = 'not json at all'
        self.reply(200, {'object': 'chat.completion', 'model': model,
                         'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': content}}],
                         'usage': usage})
        if model == 'last-answer':
            # Answer once (billed), then go away: later connections are refused.
            self.wfile.flush()
            os._exit(0)


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
server.timeout = 30
print(server.server_address[1], flush=True)
while True:
    before = time.monotonic()
    server.handle_request()
    if time.monotonic() - before >= 30:
        break
