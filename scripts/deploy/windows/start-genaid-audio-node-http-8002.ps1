Set-Location 'Q:\deploy\gen-audio'
& 'Q:\deploy\gen-audio\.venv\Scripts\python.exe' -c 'from gen_audio.node_http import serve_loopback; serve_loopback().serve_forever()' > 'Q:\deploy\gen-audio-run\genaid-audio-node-http.out.log' 2> 'Q:\deploy\gen-audio-run\genaid-audio-node-http.err.log'
