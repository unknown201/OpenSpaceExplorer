// Shape compressed HTTP bytes across both the page and service workers with one shared link budget.
import http from 'node:http';
import https from 'node:https';

export async function networkProxy(target, mbps, latencyMs = 30) {
  const upstream = new URL(target), transport = upstream.protocol === 'https:' ? https : http;
  const agent = new transport.Agent({keepAlive:true});
  const streams = new Set(), sockets = new Set();
  const rate = mbps * 1e6 / 8;
  let tokens = 0, at = performance.now(), transferredBytes = 0;
  const timer = setInterval(() => {
    const now=performance.now();tokens=Math.min(rate*0.02,tokens+rate*(now-at)/1000);at=now;
    const ready=[...streams].filter(s=>!s.blocked && s.queue.length).length;
    const share=Math.max(1,Math.floor(tokens/Math.max(1,ready)));
    for (const stream of streams) {
      if (stream.out.destroyed) {stream.source.destroy();streams.delete(stream);continue;}
      if (stream.ended && !stream.queue.length) {stream.out.end();streams.delete(stream);continue;}
      if (stream.blocked) continue;
      let allowance=share;
      while (stream.queue.length && tokens>=1 && allowance>=1 && !stream.blocked) {
        const first=stream.queue[0];
        const n=Math.min(first.length,Math.floor(tokens),allowance,64*1024);
        tokens-=n;allowance-=n;stream.bytes-=n;transferredBytes+=n;
        if(n===first.length)stream.queue.shift();else stream.queue[0]=first.subarray(n);
        stream.blocked=!stream.out.write(first.subarray(0,n));
        if(stream.bytes<128*1024)stream.source.resume();
      }
      if (stream.ended && !stream.queue.length) {stream.out.end();streams.delete(stream);}
    }
  },1);
  const server=http.createServer((req,res) => {
    setTimeout(() => {
      if(res.destroyed)return;
      const url=new URL(req.url,upstream);
      const request=transport.request(url,{method:req.method,headers:{...req.headers,host:upstream.host},agent},response => {
        response.pause();
        setTimeout(() => {
          if(res.destroyed){response.destroy();return;}
          res.writeHead(response.statusCode,response.headers);
          if(req.method==='HEAD' || [204,304].includes(response.statusCode)) {response.resume();res.end();return;}
          const stream={source:response,out:res,queue:[],bytes:0,blocked:false,ended:false};
          streams.add(stream);
          res.on('drain',()=>{stream.blocked=false;});
          response.on('data',chunk=>{stream.queue.push(chunk);stream.bytes+=chunk.length;if(stream.bytes>=128*1024)response.pause();});
          response.on('end',()=>{stream.ended=true;});
          response.on('error',()=>res.destroy());response.resume();
        },latencyMs/2);
      });
      request.on('error',()=>{if(!res.headersSent)res.writeHead(502);res.end();});
      req.pipe(request);
    },latencyMs/2);
  });
  // Campaign traffic is forwarded normally; this profiler measures the shared HTTP asset budget.
  server.on('upgrade',(req,socket,head) => {
    const request=transport.request(new URL(req.url,upstream),{headers:{...req.headers,host:upstream.host}});
    request.on('upgrade',(response,remote,remoteHead) => {
      socket.write(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\n`+
        Object.entries(response.headers).map(([k,v])=>`${k}: ${v}\r\n`).join('')+'\r\n');
      if(head.length)remote.write(head);if(remoteHead.length)socket.write(remoteHead);
      socket.pipe(remote).pipe(socket);socket.on('close',()=>remote.destroy());remote.on('error',()=>socket.destroy());
    });
    request.on('error',()=>socket.destroy());request.end();
  });
  server.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket));});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  return {base:`http://127.0.0.1:${server.address().port}`,get transferredBytes() {return transferredBytes;},async close() {
    clearInterval(timer);for(const socket of sockets)socket.destroy();agent.destroy();
    await new Promise(resolve=>server.close(resolve));
  }};
}
