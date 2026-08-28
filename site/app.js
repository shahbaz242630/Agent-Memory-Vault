// Zaaheen landing page. No framework, no tracking, no third-party scripts.
(function () {
  var tabs = document.querySelectorAll('.tab');
  var panels = document.querySelectorAll('.panel');

  function show(os) {
    tabs.forEach(function (t) {
      t.setAttribute('aria-selected', String(t.dataset.os === os));
    });
    panels.forEach(function (p) {
      p.classList.toggle('hidden', p.dataset.os !== os);
    });
  }

  tabs.forEach(function (t) {
    t.addEventListener('click', function () { show(t.dataset.os); });
  });

  // Preselect the visitor's platform. A Mac visitor should land on the Mac tab
  // and see "coming soon" -- not on Windows, concluding it is Windows-only.
  var ua = navigator.userAgent;
  var guess = /Macintosh|Mac OS X/i.test(ua) ? 'mac'
            : /Linux|X11/i.test(ua) && !/Android/i.test(ua) ? 'linux'
            : 'win';
  show(guess);

  // The download href is held in data-href until the file is actually live, so
  // the button cannot 404 while the bucket is still empty.
  var win = document.getElementById('win-dl');
  if (win && win.dataset.href) { win.href = win.dataset.href; }

  // Notify form. NOT YET WIRED: needs an endpoint (a small Cloudflare Worker
  // writing to KV or D1). Until then it acknowledges locally rather than
  // silently dropping an address, which would be worse than saying nothing.
  document.querySelectorAll('.notify').forEach(function (form) {
    form.addEventListener('submit', function (e) {
      e.preventDefault();
      var note = form.parentElement.querySelector('.form-note');
      note.hidden = false;
      note.textContent = 'Thanks — we will let you know about ' +
        form.dataset.platform + '.';
      form.reset();
    });
  });
})();
