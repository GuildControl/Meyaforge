(() => {
  const T = window.__TAURI__;
  const invoke = T ? T.core.invoke : async () => null;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  let currentProject = null;
  let toolsLatestMap = {};
  let currentCategory = "all";
  let isCheckingUpdates = false;
  let currentDevInfo = null;

  /* ---------- окно ---------- */
  if (T) {
    const w = T.window.getCurrentWindow();
    $("#wMin").onclick = () => w.minimize();
    $("#wMax").onclick = () => w.toggleMaximize();
    $("#wClose").onclick = () => w.close();
  }

  /* ---------- МУЛЬТИЯЗЫЧНОСТЬ (i18n) ---------- */
  let currentLang = localStorage.getItem("meya_lang") || "ru";
  let langDict = {};
  let sortedDictKeys = [];
  const originalRuDOMTexts = new Map();

  function translateText(str) {
    if (!str) return str;
    const trimmed = str.trim();
    const norm = trimmed.replace(/\s+/g, " ");
    if (langDict[trimmed] && langDict[trimmed] !== trimmed) return str.replace(trimmed, langDict[trimmed]);
    if (langDict[norm] && langDict[norm] !== norm) return langDict[norm];
    if (langDict[str] && langDict[str] !== str) return langDict[str];
    let replaced = str;
    for (const k of sortedDictKeys) {
      if (k.length >= 2 && replaced.includes(k)) {
        replaced = replaced.replaceAll(k, langDict[k]);
      }
    }
    return replaced;
  }

  function t(str) {
    return translateText(str);
  }

  async function loadLanguage(lang) {
    try {
      const res = await fetch(`lang/${lang}.json`);
      if (!res.ok) throw new Error("Status " + res.status);
      langDict = await res.json();
      sortedDictKeys = Object.keys(langDict)
        .filter((k) => langDict[k] && langDict[k] !== k)
        .sort((a, b) => b.length - a.length);
      currentLang = lang;
      localStorage.setItem("meya_lang", lang);
      applyLanguage();
      updateLangUiButtons();

      // Синхронизируем язык с Telegram ботом, если конфиг загружен
      try {
        const cfg = await invoke("tg_get_config");
        if (cfg && cfg.lang !== lang) {
          cfg.lang = lang;
          await invoke("tg_save_config", { config: cfg });
        }
      } catch (ignored) {}
    } catch (e) {
      console.warn("Could not load language:", lang, e);
    }
  }

  function applyLanguage() {
    // 1. По data-i18n
    $$("[data-i18n]").forEach(el => {
      const key = el.dataset.i18n;
      if (langDict[key]) {
        el.textContent = langDict[key];
      }
    });

    // 2. Глубокий обход DOM дерева для текстовых узлов
    function walkTextNodes(node) {
      if (node.nodeType === Node.TEXT_NODE) {
        const val = node.nodeValue;
        if (!val) return;
        const txt = val.trim();
        if (txt) {
          if (!originalRuDOMTexts.has(node)) {
            originalRuDOMTexts.set(node, val);
          }
          const orig = originalRuDOMTexts.get(node);
          const translated = translateText(orig);
          if (node.nodeValue !== translated) {
            node.nodeValue = translated;
          }
        }
      } else if (node.nodeType === Node.ELEMENT_NODE && node.tagName !== "SCRIPT" && node.tagName !== "STYLE") {
        // Placeholders, titles, optgroup labels
        if (node.placeholder || node.dataset.origPlaceholder) {
          if (!node.dataset.origPlaceholder) node.dataset.origPlaceholder = node.placeholder;
          const orig = node.dataset.origPlaceholder;
          const translated = translateText(orig);
          if (node.placeholder !== translated) node.placeholder = translated;
        }
        if (node.title || node.dataset.origTitle) {
          if (!node.dataset.origTitle) node.dataset.origTitle = node.title;
          const orig = node.dataset.origTitle;
          const translated = translateText(orig);
          if (node.title !== translated) node.title = translated;
        }
        if (node.label || node.dataset.origLabel) {
          if (!node.dataset.origLabel) node.dataset.origLabel = node.label;
          const orig = node.dataset.origLabel;
          const translated = translateText(orig);
          if (node.label !== translated) node.label = translated;
        }
        node.childNodes.forEach(walkTextNodes);
      }
    }

    walkTextNodes(document.body);
  }

  function updateLangUiButtons() {
    $$(".btn-lang").forEach(btn => {
      const l = btn.dataset.lang;
      if (l === currentLang) {
        btn.classList.add("active");
        btn.style.background = "rgba(142,197,255,.2)";
        btn.style.color = "var(--text)";
      } else {
        btn.classList.remove("active");
        btn.style.background = "transparent";
        btn.style.color = "var(--muted)";
      }
    });

    $$(".btn-set-lang").forEach(btn => {
      const l = btn.dataset.setLang;
      if (l === currentLang) {
        btn.className = "btn primary btn-set-lang";
      } else {
        btn.className = "btn ghost btn-set-lang";
      }
    });
  }

  // Клики по переключателям языков
  $$(".btn-lang").forEach(btn => {
    btn.onclick = () => {
      const l = btn.dataset.lang;
      if (l && l !== currentLang) {
        loadLanguage(l);
        toast(l === "en" ? "Language switched to English" : "Язык переключен на русский", "pink");
      }
    };
  });

  $$(".btn-set-lang").forEach(btn => {
    btn.onclick = () => {
      const l = btn.dataset.setLang;
      if (l) {
        loadLanguage(l);
        toast(l === "en" ? "Language switched to English" : "Язык переключен на русский", "pink");
      }
    };
  });

  // Загружаем язык при старте
  loadLanguage(currentLang);

  // Наблюдатель за динамически добавляемыми элементами в DOM
  let isTranslating = false;
  let translateTimer = null;
  const domObserver = new MutationObserver((mutations) => {
    if (!isTranslating) {
      if (translateTimer) clearTimeout(translateTimer);
      translateTimer = setTimeout(() => {
        isTranslating = true;
        try {
          applyLanguage();
        } finally {
          setTimeout(() => { isTranslating = false; }, 50);
        }
      }, 50);
    }
  });
  domObserver.observe(document.body, { childList: true, subtree: true });

  /* ---------- фон: частицы ---------- */
  const cv = $("#bg"), cx = cv.getContext("2d");
  let pts = [];
  const resize = () => {
    cv.width = innerWidth; cv.height = innerHeight;
    const n = Math.min(70, Math.floor((innerWidth * innerHeight) / 22000));
    pts = Array.from({ length: n }, () => ({
      x: Math.random() * cv.width, y: Math.random() * cv.height,
      vx: (Math.random() - .5) * .25, vy: (Math.random() - .5) * .25,
      r: Math.random() * 1.6 + .4, c: Math.random() < .5 ? "245,163,199" : "142,197,255",
    }));
  };
  resize(); addEventListener("resize", resize);
  (function draw() {
    const g = cx.createRadialGradient(cv.width * .7, cv.height * .2, 0, cv.width * .7, cv.height * .2, cv.width);
    g.addColorStop(0, "#14245a"); g.addColorStop(1, "#0b1638");
    cx.fillStyle = g; cx.fillRect(0, 0, cv.width, cv.height);
    for (let i = 0; i < pts.length; i++) {
      const p = pts[i];
      p.x = (p.x + p.vx + cv.width) % cv.width; p.y = (p.y + p.vy + cv.height) % cv.height;
      cx.fillStyle = `rgba(${p.c},.7)`; cx.beginPath(); cx.arc(p.x, p.y, p.r, 0, 7); cx.fill();
      for (let j = i + 1; j < pts.length; j++) {
        const q = pts[j], d = Math.hypot(p.x - q.x, p.y - q.y);
        if (d < 120) { cx.strokeStyle = `rgba(169,139,255,${.12 * (1 - d / 120)})`; cx.beginPath(); cx.moveTo(p.x, p.y); cx.lineTo(q.x, q.y); cx.stroke(); }
      }
    }
    if (!document.hidden) requestAnimationFrame(draw); else setTimeout(() => requestAnimationFrame(draw), 500);
  })();

  /* ---------- утилиты UI ---------- */
  function toast(msg, kind = "") {
    const tEl = document.createElement("div");
    tEl.className = "toast " + kind; tEl.textContent = t(msg); $("#toasts").append(tEl);
    setTimeout(() => { tEl.classList.add("out"); setTimeout(() => tEl.remove(), 300); }, 3400);
  }

  function formatBytes(bytes) {
    if (!bytes || bytes <= 0) return currentLang === "en" ? "0 B" : "0 Б";
    const k = 1024, dm = 2;
    const sizes = currentLang === "en" ? ["B", "KB", "MB", "GB", "TB"] : ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + " " + sizes[i];
  }

  function formatDate(ts) {
    if (!ts) return "—";
    const d = new Date(ts * 1000);
    return d.toLocaleDateString() + " " + d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
  }

  document.addEventListener("click", (e) => {
    const b = e.target.closest(".btn"); if (!b) return;
    const r = b.getBoundingClientRect(), s = Math.max(r.width, r.height), sp = document.createElement("span");
    sp.className = "ripple"; sp.style.cssText = `width:${s}px;height:${s}px;left:${e.clientX - r.left - s / 2}px;top:${e.clientY - r.top - s / 2}px`;
    b.append(sp); setTimeout(() => sp.remove(), 600);
  });

  $$(".card").forEach((c) => {
    c.addEventListener("mousemove", (e) => {
      const r = c.getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top;
      c.style.setProperty("--mx", x + "px"); c.style.setProperty("--my", y + "px");
      c.style.transform = `perspective(700px) rotateX(${(r.height / 2 - y) / 18}deg) rotateY(${(x - r.width / 2) / 18}deg) translateY(-3px)`;
    });
    c.addEventListener("mouseleave", () => (c.style.transform = ""));
    c.addEventListener("click", () => go(c.dataset.go));
  });

  /* ---------- навигация ---------- */
  const ind = $(".nav-indicator");
  function go(page) {
    $$(".nav").forEach((n) => n.classList.toggle("active", n.dataset.page === page));
    $$(".page").forEach((p) => p.classList.toggle("active", p.dataset.page === page));
    const a = $(".nav.active"); if (a) { ind.style.top = a.offsetTop + "px"; ind.style.height = a.offsetHeight + "px"; }

    if (page === "projects") refreshProjects();
    if (page === "partitions") loadCurrentProjectDetails();
    if (page === "files") initFileManager();
    if (page === "tools") refreshTools();
    if (page === "device") { checkDeviceQuick(); loadSavedDevicesCount(); }
    if (page === "settings") loadTelegramConfig();
    if (page === "unpack") initRepackPage();
    if (page === "debloat") initDebloatPage();
    if (page === "apk") initApkKitchen();
    if (page === "port") initPortingPage();

    // Применяем перевод ко всем динамическим блокам открытой страницы
    if (currentLang === "en") {
      setTimeout(applyLanguage, 50);
    }
  }
  $$(".nav").forEach((n) => (n.onclick = () => go(n.dataset.page)));
  addEventListener("resize", () => go($(".nav.active").dataset.page));
  $("#btnGoToProjects").onclick = () => go("projects");

  /* ---------- терминал ---------- */
  const body = $("#termBody"), tin = $("#termIn"), state = $("#termState");
  const hist = []; let hi = 0, busy = false;
  function line(text, cls = "") {
    const d = document.createElement("div"); d.className = "l " + cls; d.textContent = t(text); body.append(d);
    while (body.childElementCount > 2000) body.firstChild.remove();
    body.scrollTop = body.scrollHeight;
  }
  function setBusy(b) {
    busy = b;
    state.textContent = b ? t("выполняется…") : t("готов");
    state.classList.toggle("busy", b);
  }
  
  if (T) {
    T.event.listen("term", ({ payload }) => {
      if (payload.kind === "exit") {
        const exitText = currentLang === "en" ? `[finished, code ${payload.text}]` : `[завершено, код ${payload.text}]`;
        line(exitText, payload.text === "0" ? "sys" : "err");
        setBusy(false);
      }
      else line(payload.text, payload.kind === "err" ? "err" : (payload.kind === "sys" ? "sys" : ""));
    });

    T.event.listen("task", ({ payload }) => {
      const tb = $("#topTaskBar");
      if (payload.active) {
        tb.classList.remove("hidden");
        $("#topTaskLabel").textContent = t(payload.label);
        $("#topTaskPct").textContent = payload.pct >= 0 ? `${payload.pct}%` : "";
      } else {
        tb.classList.add("hidden");
      }
    });

    T.event.listen("changed", () => {
      if ($(".page.active").dataset.page === "projects") refreshProjects();
      if ($(".page.active").dataset.page === "partitions") loadCurrentProjectDetails();
      if ($(".page.active").dataset.page === "tools") refreshTools();
      if ($(".page.active").dataset.page === "apk") apkRefreshAll();
    });
  }

  $("#termForm").onsubmit = async (e) => {
    e.preventDefault();
    const cmd = tin.value.trim(); if (!cmd) return;
    hist.push(cmd); hi = hist.length; tin.value = "";
    line("» " + cmd, "cmd");
    if (cmd === "clear" || cmd === "cls") { body.innerHTML = ""; return; }
    if (busy) { line("Дождитесь завершения текущей команды", "err"); return; }
    setBusy(true);
    try { await invoke("run_command", { cmd }); } catch (err) { line(String(err), "err"); setBusy(false); }
  };
  tin.addEventListener("keydown", (e) => {
    if (e.key === "ArrowUp" && hi > 0) { tin.value = hist[--hi]; e.preventDefault(); }
    if (e.key === "ArrowDown") { hi = Math.min(hist.length, hi + 1); tin.value = hist[hi] || ""; }
  });
  $("#termClear").onclick = () => (body.innerHTML = "");
  $("#termToggle").onclick = () => $("#terminal").classList.toggle("collapsed");

  /* ---------- проекты ---------- */
  async function refreshProjects() {
    const list = $("#projectsList");
    try {
      const items = await invoke("list_projects");
      if (!items || items.length === 0) {
        list.innerHTML = `<div class="empty-placeholder" style="grid-column: 1 / -1">
          <div class="ic">${icon('folder')}</div>
          <h3>Нет созданных проектов</h3>
          <p class="muted">Создайте ваш первый проект прошивки для распаковки разделов.</p>
          <button class="btn primary" onclick="document.getElementById('btnNewProject').click()">+ Создать проект</button>
        </div>`;
        return;
      }
      list.innerHTML = "";
      items.forEach(item => {
        const card = document.createElement("div");
        card.className = "proj-card" + (currentProject === item.meta.name ? " active-proj" : "");
        card.innerHTML = `
          <div class="proj-head">
            <h3>${item.meta.name}</h3>
            <span class="proj-date">${formatDate(item.meta.created)}</span>
          </div>
          <div class="proj-stats">
            <span>Образов: <b>${item.parts}</b></span>
            <span>Источник: <b>${item.meta.source ? 'Загружен' : 'Не задан'}</b></span>
          </div>
          <div class="proj-footer">
            <button class="btn sm danger btn-del-proj" title="Удалить проект">Удалить</button>
            <button class="btn sm primary btn-open-proj">Открыть разделы →</button>
          </div>
        `;
        card.querySelector(".btn-open-proj").onclick = (e) => {
          e.stopPropagation();
          selectProject(item.meta.name);
        };
        card.querySelector(".btn-del-proj").onclick = (e) => {
          e.stopPropagation();
          showConfirmDialog({
            title: `Удаление проекта ${item.meta.name}`,
            desc: `Вы действительно хотите безвозвратно удалить проект <b>${item.meta.name}</b> со всеми распакованными файлами и образами?`,
            iconName: "trash",
            okText: "Удалить проект",
            okDanger: true,
            onConfirm: async () => {
              try {
                await invoke("delete_project", { name: item.meta.name });
                if (currentProject === item.meta.name) currentProject = null;
                toast(`Проект ${item.meta.name} удален`);
                refreshProjects();
              } catch (err) { toast("Ошибка: " + err); }
            }
          });
        };
        card.onclick = () => selectProject(item.meta.name);
        list.appendChild(card);
      });
    } catch (e) {
      list.innerHTML = `<div class="empty-state">Ошибка загрузки проектов: ${e}</div>`;
    }
  }

  function selectProject(name) {
    currentProject = name;
    toast(`Выбран проект: ${name}`, "pink");
    go("partitions");
  }

  // Создание проекта
  const newProjModal = $("#newProjectModal");
  $("#btnNewProject").onclick = () => {
    $("#inputProjName").value = "";
    $("#newProjectErr").textContent = "";
    newProjModal.classList.remove("hidden");
    setTimeout(() => $("#inputProjName").focus(), 100);
  };
  $("#btnCancelNewProj").onclick = () => newProjModal.classList.add("hidden");
  $("#newProjectForm").onsubmit = async (e) => {
    e.preventDefault();
    const name = $("#inputProjName").value.trim();
    try {
      await invoke("create_project", { name });
      newProjModal.classList.add("hidden");
      toast(`Проект ${name} успешно создан!`, "pink");
      currentProject = name;
      go("partitions");
    } catch (err) {
      $("#newProjectErr").textContent = String(err);
    }
  };

  /* ---------- разделы проекта ---------- */
  async function loadCurrentProjectDetails() {
    const noSel = $("#noProjectSelected");
    const content = $("#partitionsContent");
    const btnImport = $("#btnImportFirmware");
    const btnExtractAll = $("#btnExtractAllParts");
    const btnOpenFolder = $("#btnOpenProjFolder");

    if (!currentProject) {
      noSel.classList.remove("hidden");
      content.classList.add("hidden");
      btnImport.disabled = true;
      if (btnExtractAll) btnExtractAll.disabled = true;
      btnOpenFolder.disabled = true;
      $("#activeProjectCrumbs").textContent = "Проект не выбран";
      $("#partitionsTitle").textContent = "Управление разделами";
      return;
    }

    $("#activeProjectCrumbs").textContent = `Проект: ${currentProject}`;
    $("#partitionsTitle").textContent = currentProject;
    btnImport.disabled = false;
    if (btnExtractAll) btnExtractAll.disabled = false;
    btnOpenFolder.disabled = false;

    try {
      const d = await invoke("get_project", { name: currentProject });
      noSel.classList.add("hidden");
      content.classList.remove("hidden");

      $("#projSourceText").textContent = d.meta.source ? d.meta.source.split(/[\\/]/).pop() : "не импортирован";
      $("#projPartsCount").textContent = d.parts.length;

      const tbody = $("#partitionsTbody");
      tbody.innerHTML = "";

      if (d.parts.length === 0) {
        tbody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:30px;color:var(--muted)">В проекте нет образов разделов. Нажмите «Импорт прошивки / образа» вверху.</td></tr>`;
        return;
      }

      d.parts.forEach(p => {
        const tr = document.createElement("tr");
        let badgeText = p.kind;
        if (p.kind === "sparse" && p.inner) {
          badgeText = `sparse (${p.inner})`;
        }

        const isBoot = ["boot", "vendor_boot", "recovery", "init_boot"].includes(p.name.toLowerCase()) || p.kind === "boot" || p.kind === "vendor_boot";
        const isLk = ["lk", "lk2", "abl", "xbl", "bootloader"].includes(p.name.toLowerCase()) || p.name.toLowerCase().startsWith("lk");
        const canExtract = ["ext4", "erofs", "super"].includes(p.kind) || (p.kind === "sparse" && ["ext4", "erofs"].includes(p.inner)) || isBoot;
        const canUnsparse = p.kind === "sparse";

        tr.innerHTML = `
          <td><b>${p.name}</b></td>
          <td><span class="badge ${p.kind}">${badgeText}</span></td>
          <td>${formatBytes(p.size)}</td>
          <td>${p.extracted ? '<span class="tag-extracted"><i></i> Разобран</span>' : '<span class="tag-not-extracted">Не разобран</span>'}</td>
          <td style="text-align:right">
            ${isLk ? `<button class="btn sm ghost btn-lk-analyze" title="Извлечь Fastboot OEM команды">OEM LK</button> ` : ''}
            ${isBoot ? `<button class="btn sm ghost btn-boot-patch" title="Патчер ядра">Патчи</button> ` : ''}
            ${canUnsparse ? `<button class="btn sm ghost btn-unsparse">В raw</button> ` : ''}
            ${canExtract ? `<button class="btn sm primary btn-extract">${p.extracted ? 'Переразобрать' : (isBoot ? 'Разобрать boot' : 'Разобрать')}</button> ` : ''}
            ${p.extracted ? `<button class="btn sm ghost btn-open-extracted">Папка</button> ` : ''}
            <button class="btn sm danger btn-part-del" title="Удалить образ или папку раздела">Удалить</button>
          </td>
        `;

        const bLk = tr.querySelector(".btn-lk-analyze");
        if (bLk) {
          bLk.onclick = () => {
            go("bootapk");
            switchPatchTab("lk");
            analyzeLkPartition(p.name);
          };
        }

        const bBoot = tr.querySelector(".btn-boot-patch");
        if (bBoot) {
          bBoot.onclick = () => {
            go("bootapk");
            switchPatchTab("boot");
            $("#inputBootExtractedName").value = p.name;
            inspectBootFolder(p.name);
          };
        }

        const bExtract = tr.querySelector(".btn-extract");
        if (bExtract) {
          bExtract.onclick = async () => {
            try {
              await invoke("extract_partition", { name: currentProject, part: p.name });
              toast(`Запущена разборка ${p.name}...`);
            } catch (err) { toast("Ошибка: " + err); }
          };
        }

        const bUnsparse = tr.querySelector(".btn-unsparse");
        if (bUnsparse) {
          bUnsparse.onclick = async () => {
            try {
              await invoke("unsparse_partition", { name: currentProject, part: p.name });
              toast(`Запущено преобразование sparse ${p.name}...`);
            } catch (err) { toast("Ошибка: " + err); }
          };
        }

        const bOpenExt = tr.querySelector(".btn-open-extracted");
        if (bOpenExt) {
          bOpenExt.onclick = () => invoke("open_in_explorer", { name: currentProject, sub: `extracted/${p.name}` });
        }

        const bDelPart = tr.querySelector(".btn-part-del");
        if (bDelPart) {
          bDelPart.onclick = () => {
            showConfirmDialog({
              title: `Удаление раздела ${p.name}`,
              desc: `Удалить файлы раздела <b>${p.name}</b> (образ images/${p.name}.img и папку extracted/${p.name}) из проекта <b>${currentProject}</b>?<br><br><span style="color:#ff8fab">Данные раздела будут удалены с диска только после нажатия «Удалить раздел».</span>`,
              iconName: "trash",
              okText: "Удалить раздел",
              okDanger: true,
              onConfirm: async () => {
                try {
                  await invoke("delete_partition", { name: currentProject, part: p.name });
                  toast(`Раздел ${p.name} удален`);
                  loadCurrentProjectDetails();
                } catch (err) {
                  toast("Ошибка удаления раздела: " + err);
                }
              }
            });
          };
        }

        tbody.appendChild(tr);
      });
    } catch (e) {
      toast("Ошибка загрузки деталей проекта: " + e);
    }
  }

  // Импорт прошивки с модальным окном параметров
  let pendingImportPath = null;

  function openImportOptions(filePath) {
    pendingImportPath = filePath;
    const fname = filePath.split(/[\\/]/).pop();
    $("#importOptFileName").textContent = fname;
    $("#importOptionsModal").classList.remove("hidden");
  }

  $("#btnCancelImportOpt").onclick = () => {
    $("#importOptionsModal").classList.add("hidden");
    pendingImportPath = null;
  };

  $("#btnConfirmImportOpt").onclick = async () => {
    if (!pendingImportPath || !currentProject) return;
    const path = pendingImportPath;
    pendingImportPath = null;
    $("#importOptionsModal").classList.add("hidden");

    const options = {
      auto_extract_parts: $("#optAutoExtract").checked,
      delete_intermediate: $("#optDeleteIntermediate").checked,
      delete_archive: $("#optDeleteArchive").checked,
      delete_raw_imgs: $("#optDeleteRawImgs").checked,
    };

    try {
      await invoke("import_firmware", { name: currentProject, path, options });
      toast("Импорт и разборка прошивки запущены в фоне...", "pink");
    } catch (e) {
      toast("Ошибка импорта: " + e);
    }
  };

  $("#btnImportFirmware").onclick = async () => {
    if (!currentProject) return;
    if (T && T.dialog) {
      try {
        const selected = await T.dialog.open({
          multiple: false,
          filters: [
            { name: 'Все прошивки и архивы', extensions: ['zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'md5', 'bin', 'img', 'lz4', 'zst'] },
            { name: 'Архивы (zip, rar, 7z, tar.md5)', extensions: ['zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'md5'] },
            { name: 'Образы и контейнеры (payload.bin, super.img, *.img)', extensions: ['bin', 'img', 'lz4', 'zst'] },
            { name: 'Все файлы', extensions: ['*'] }
          ]
        });
        if (selected) {
          openImportOptions(selected);
        }
      } catch (e) {
        toast("Ошибка выбора файла: " + e);
      }
    } else {
      const path = prompt("Введите абсолютный путь к файлу прошивки (zip, rar, 7z, tar.md5, payload.bin, super.img или *.img):");
      if (path) {
        openImportOptions(path);
      }
    }
  };

  $("#btnExtractAllParts").onclick = async () => {
    if (!currentProject) return;
    askDeviceConfirmation(
      "Разборка всех разделов",
      `Вы действительно хотите запустить автоматическую разборку всех разделов проекта <b>${currentProject}</b> в папки extracted/?<br><br>
      Будут распакованы ext4 и erofs файловые системы, а также boot/recovery на kernel, dtb и дерево ramdisk.`,
      async () => {
        try {
          await invoke("extract_all_partitions", { name: currentProject, deleteRawImgs: false });
          toast("Разборка всех разделов запущена в фоне...", "pink");
        } catch (e) {
          toast("Ошибка: " + e);
        }
      }
    );
  };

  $("#btnOpenProjFolder").onclick = () => {
    if (currentProject) invoke("open_in_explorer", { name: currentProject, sub: "" });
  };

  /* ---------- чтение и диагностика устройства ---------- */
  async function checkDeviceQuick() {
    try {
      const mode = await invoke("device_detect");
      if (mode && mode.mode !== "none") {
        $("#devMainTitle").textContent = `${mode.description} (${mode.serial})`;
      }
    } catch (_) {}
  }

  // Универсальное модальное окно подтверждения удаления / опасных действий
  let pendingModalConfirm = null;

  function showConfirmDialog({ title, desc, iconName = "trash", okText = "Удалить", okDanger = true, onConfirm }) {
    $("#confirmModalIcon").innerHTML = icon(iconName, 38);
    $("#confirmModalTitle").textContent = title;
    $("#confirmModalDesc").innerHTML = desc;
    const okBtn = $("#btnOkConfirmModal");
    okBtn.textContent = okText;
    if (okDanger) {
      okBtn.className = "btn danger";
    } else {
      okBtn.className = "btn primary";
    }
    pendingModalConfirm = onConfirm;
    $("#universalConfirmModal").classList.remove("hidden");
  }

  $("#btnCancelConfirmModal").onclick = () => {
    $("#universalConfirmModal").classList.add("hidden");
    pendingModalConfirm = null;
  };

  $("#btnOkConfirmModal").onclick = async () => {
    $("#universalConfirmModal").classList.add("hidden");
    if (pendingModalConfirm) {
      const action = pendingModalConfirm;
      pendingModalConfirm = null;
      try {
        await action();
      } catch (err) {
        console.error("Action error:", err);
      }
    }
  };

  // Окно предупреждения перед операциями
  let pendingDeviceAction = null;

  function askDeviceConfirmation(title, desc, onConfirm) {
    $("#warnModalTitle").textContent = title;
    if (desc) $("#warnModalDesc").innerHTML = desc;
    pendingDeviceAction = onConfirm;
    $("#deviceWarningModal").classList.remove("hidden");
  }

  $("#btnCancelDeviceWarn").onclick = () => {
    $("#deviceWarningModal").classList.add("hidden");
    pendingDeviceAction = null;
  };

  $("#btnConfirmDeviceWarn").onclick = () => {
    $("#deviceWarningModal").classList.add("hidden");
    if (pendingDeviceAction) {
      const action = pendingDeviceAction;
      pendingDeviceAction = null;
      action();
    }
  };

  $("#btnScanDevice").onclick = () => {
    askDeviceConfirmation(
      "Сквозной сбор данных (System -> Bootloader -> System)",
      `MEYAFORGE выполнит глубокий цикл опроса устройства:<br>
      <b>1.</b> Считывание свойств Android, точек монтирования и ядра через ADB.<br>
      <b>2.</b> Перезагрузка в режим <b>Fastboot / Bootloader</b> для снятия аппаратных таблиц, разметки и статуса блокировки загрузчика.<br>
      <b>3.</b> Автоматический возврат (перезагрузка) в обычный режим Android OS.<br><br>
      <span style="color:#ff8fab">${icon('alert', 15)} Не отключайте кабель USB и не трогайте телефон до завершения всего цикла!</span>`,
      executeScanDevice
    );
  };

  let currentScanProfile = null;

  function displayDeviceInfo(info, scanProfile) {
    if (!info) return;
    currentDevInfo = info;
    currentScanProfile = scanProfile || null;

    $("#deviceScanningBox").classList.add("hidden");
    $("#deviceEmptyPlaceholder").classList.add("hidden");
    $("#deviceDetailsContent").classList.remove("hidden");

    // Баннер об ошибке сканирования (если сбой)
    const errBanner = $("#devScanErrorBanner");
    const errDesc = $("#devScanErrorDesc");
    const btnSavePartial = $("#btnSavePartialScan");

    if (scanProfile && !scanProfile.success) {
      errBanner.classList.remove("hidden");
      errDesc.textContent = `Сбой на этапе «${scanProfile.failure_stage || "Неизвестный этап"}». ${scanProfile.failure_error || "Неизвестная ошибка"}`;
      btnSavePartial.classList.remove("hidden");
      btnSavePartial.onclick = async () => {
        try {
          await invoke("save_device_profile", { profile: scanProfile });
          toast("Профиль устройства успешно сохранён в хранилище!", "pink");
          btnSavePartial.classList.add("hidden");
          loadSavedDevicesCount();
        } catch (e) {
          toast("Ошибка сохранения: " + e);
        }
      };
    } else {
      errBanner.classList.add("hidden");
    }

    // Режим
    const badge = $("#devModeBadge");
    badge.className = `dev-mode-badge ${info.mode.mode}`;
    badge.textContent = `${info.mode.mode.toUpperCase()}`;
    $("#devMainTitle").textContent = `${info.brand || ''} ${info.model || 'Android Device'} ${info.product ? '(' + info.product + ')' : ''}`.trim();
    $("#devSerial").textContent = info.mode.serial || '—';
    $("#devModeDesc").textContent = info.mode.description || '—';

    // Спецификации
    $("#specBrand").textContent = `${info.brand || '—'} / ${info.manufacturer || '—'}`;
    $("#specModel").textContent = `${info.model || '—'} (${info.device || '—'})`;
    $("#specSoc").textContent = info.soc || '—';
    $("#specAbi").textContent = info.cpu_abi || '—';
    $("#specAndroid").textContent = info.android_version ? `Android ${info.android_version} (SDK ${info.sdk_version})` : '—';
    $("#specSecurity").textContent = info.security_patch || '—';
    $("#specSlots").textContent = info.is_ab ? `A/B (Активный слот: ${info.slot ? info.slot.toUpperCase() : '—'})` : 'A-only (Non A/B)';
    $("#specDynamic").textContent = info.is_dynamic ? 'Да (Super partition)' : 'Нет (Стандартная разметка)';
    $("#specBootloader").textContent = info.bootloader_status || '—';
    $("#specRecovery").textContent = info.recovery_type || '—';
    $("#specSelinux").textContent = info.selinux || '—';
    $("#specRoot").textContent = info.is_root ? 'Да (UID=0 / SU доступен)' : 'Нет (Не root)';
    $("#specDisplay").textContent = info.display || '—';
    $("#specBaseband").textContent = info.baseband || '—';

    // Батарея, ядро, RAM, cmdline, OEM
    $("#specKernel").textContent = info.kernel || '—';
    $("#specRam").textContent = info.memory_info || '—';
    $("#specBattery").textContent = info.battery || '—';
    $("#specFingerprint").textContent = info.build_fingerprint || info.build_id || '—';
    $("#specCmdline").textContent = info.kernel_cmdline || '—';

    if (info.oem_device_info && info.oem_device_info.trim().length > 0) {
      $("#specOemInfo").textContent = info.oem_device_info;
      $("#rowOemInfo").style.display = "flex";
    } else {
      $("#rowOemInfo").style.display = "none";
    }

    // Разделы устройства
    $("#devPartsCount").textContent = info.partitions.length;
    renderDevicePartitions(info.partitions);

    // Свойства getprop
    const propsCount = Object.keys(info.build_props || {}).length;
    $("#devPropsCount").textContent = propsCount;
    renderDeviceProps(info.build_props || {}, "");

    // Переменные Fastboot
    const fbVarsCount = Object.keys(info.fastboot_vars || {}).length;
    $("#devFbVarsCount").textContent = fbVarsCount;
    renderFastbootVars(info.fastboot_vars || {}, "");

    // Fstab
    $("#devFstabView").textContent = info.raw_fstab || "Конфигурация fstab не найдена или недоступна без root прав.";
    renderAndroidSettings();
    renderAndroidDiagnostics();

    const images = $("#devBootImages");
    images.replaceChildren();
    for (const [name, image] of Object.entries(info.boot_images || {})) {
      const item = document.createElement("li");
      item.textContent = image.error
        ? `${image.partition || name} — ${image.error}`
        : `${image.relative_path} — ${image.size_bytes.toLocaleString("ru-RU")} байт`;
      images.appendChild(item);
    }
    $("#devBootImageStatus").textContent = images.children.length
      ? "Пути указаны относительно каталога data рядом с приложением."
      : "В этом сканировании образы не считывались.";
  }

  function renderAndroidSettings() {
    const namespace = $("#devSettingsNamespace").value;
    const result = currentDevInfo?.android_settings?.[namespace];
    const table = $("#devSettingsTbody");
    const status = $("#devSettingsStatus");
    table.replaceChildren();
    if (!result || result.error) {
      status.textContent = result?.error || "Эта таблица не считывалась.";
      return;
    }

    const query = $("#devSettingsFilter").value.trim().toLowerCase();
    let count = 0;
    for (const line of result.output.split(/\r?\n/)) {
      const separator = line.indexOf("=");
      if (separator <= 0 || !line.toLowerCase().includes(query)) continue;
      const row = table.insertRow();
      row.insertCell().textContent = line.slice(0, separator);
      row.insertCell().textContent = line.slice(separator + 1);
      count++;
    }
    status.textContent = count ? `Записей: ${count}` : "Подходящих записей нет.";
  }

  function renderAndroidDiagnostics() {
    const result = currentDevInfo?.android_diagnostics?.[$("#devDiagnosticType").value];
    $("#devDiagnosticView").textContent = result
      ? (result.error || result.output || "Команда не вернула данных.")
      : "Данные не считывались.";
  }

  $("#devSettingsNamespace").addEventListener("change", renderAndroidSettings);
  $("#devSettingsFilter").addEventListener("input", renderAndroidSettings);
  $("#devDiagnosticType").addEventListener("change", renderAndroidDiagnostics);

  async function executeScanDevice() {
    const scanBtn = $("#btnScanDevice");
    const scanBox = $("#deviceScanningBox");
    const emptyBox = $("#deviceEmptyPlaceholder");
    const detBox = $("#deviceDetailsContent");

    scanBtn.disabled = true;
    scanBox.classList.remove("hidden");
    emptyBox.classList.add("hidden");
    detBox.classList.add("hidden");
    toast("Запущен сквозной цикл сбора данных...", "pink");

    try {
      await invoke("read_device_info_deep", { includeBootImages: $("#scanBootImages").checked });
      
      // Ждём завершения фоновой задачи и получаем данные
      const checkInterval = setInterval(async () => {
        const busy = await invoke("task_busy");
        if (!busy) {
          clearInterval(checkInterval);
          scanBtn.disabled = false;
          const profile = await invoke("get_last_scan_profile");
          const info = await invoke("get_last_device_info");
          if (info) {
            displayDeviceInfo(info, profile);
            if (profile && profile.success) {
              toast("Сквозной сбор завершен и сохранён в хранилище!", "pink");
            } else {
              toast("Сбор данных прерван с ошибкой. Предложено сохранение.", "pink");
            }
            loadSavedDevicesCount();
          } else {
            scanBox.classList.add("hidden");
            emptyBox.classList.remove("hidden");
            toast("Не удалось собрать данные устройства");
          }
        }
      }, 1500);
    } catch (e) {
      scanBox.classList.add("hidden");
      emptyBox.classList.remove("hidden");
      scanBtn.disabled = false;
      toast("Ошибка чтения устройства: " + e);
    }
  }

  /* ---------- хранилище устройств ---------- */
  async function loadSavedDevicesCount() {
    try {
      const list = await invoke("list_saved_devices");
      const count = list ? list.length : 0;
      const el = $("#savedDevicesCount");
      if (el) el.textContent = count;
    } catch (_) {}
  }

  async function openDeviceLibrary() {
    $("#deviceLibraryModal").classList.remove("hidden");
    const listEl = $("#devLibList");
    listEl.innerHTML = `<div class="spinner-mini" style="margin:20px auto"></div>`;

    try {
      const list = await invoke("list_saved_devices");
      if (!list || list.length === 0) {
        listEl.innerHTML = `
          <div class="empty-placeholder" style="padding:30px">
            <div class="ic">${icon('archive')}</div>
            <h3>Хранилище устройств пусто</h3>
            <p class="muted">После каждого сканирования устройства его профиль сохраняется сюда со всеми свойствами, разделами и fstab.</p>
          </div>
        `;
        return;
      }

      listEl.innerHTML = "";
      list.forEach(d => {
        const item = document.createElement("div");
        item.className = "spec-card";
        item.style = "display:flex;flex-direction:row;justify-content:space-between;align-items:center;padding:14px;gap:12px";

        const badgeClass = d.success ? "erofs" : "ext4";
        const badgeText = d.success ? "Полный слепок" : "С ошибкой";

        item.innerHTML = `
          <div style="flex:1;overflow:hidden">
            <div style="display:flex;align-items:center;gap:8px;margin-bottom:4px">
              <b style="font-size:14px">${d.title || `${d.brand} ${d.model}`}</b>
              <span class="badge ${badgeClass}">${badgeText}</span>
            </div>
            <div style="font-size:12px;color:var(--muted)">
              Серийный: <b style="color:var(--text)">${d.serial || '—'}</b> | Android: <b style="color:var(--text)">${d.android_version || '—'}</b> | Разделов: <b>${d.partitions_count}</b> | Свойств: <b>${d.props_count}</b>
            </div>
            <div style="font-size:11px;color:var(--muted);margin-top:2px">
              Дата: ${new Date(d.timestamp * 1000).toLocaleString()}
              ${d.failure_stage ? ` | <span style="color:#ff8fab">Сбой: ${d.failure_stage}</span>` : ''}
            </div>
          </div>
          <div style="display:flex;gap:6px">
            <button class="btn sm primary btn-load-dev">Открыть</button>
            <button class="btn sm ghost btn-del-dev" title="Удалить из хранилища">${icon('trash', 14)}</button>
          </div>
        `;

        item.querySelector(".btn-load-dev").onclick = async () => {
          try {
            const fullProfile = await invoke("get_saved_device", { id: d.id });
            $("#deviceLibraryModal").classList.add("hidden");
            displayDeviceInfo(fullProfile.data, fullProfile);
            toast(`Профиль «${d.title}» загружен из хранилища!`, "pink");
          } catch (e) {
            toast("Ошибка загрузки профиля: " + e);
          }
        };

        item.querySelector(".btn-del-dev").onclick = async () => {
          if (confirm(`Удалить профиль «${d.title}» из локального хранилища?`)) {
            try {
              await invoke("delete_saved_device", { id: d.id });
              toast("Профиль удалён");
              openDeviceLibrary();
              loadSavedDevicesCount();
            } catch (e) {
              toast("Ошибка удаления: " + e);
            }
          }
        };

        listEl.appendChild(item);
      });
    } catch (e) {
      listEl.innerHTML = `<div class="empty-state" style="color:#ff8fab">Ошибка загрузки хранилища: ${e}</div>`;
    }
  }

  $("#btnOpenDevLib").onclick = openDeviceLibrary;
  const btnEmptyDevLib = $("#btnOpenDevLibFromEmpty");
  if (btnEmptyDevLib) btnEmptyDevLib.onclick = openDeviceLibrary;
  $("#btnCloseDevLib").onclick = () => $("#deviceLibraryModal").classList.add("hidden");
  $("#btnCloseDevLibBottom").onclick = () => $("#deviceLibraryModal").classList.add("hidden");

  /* ---------- снятие логов устройства ---------- */
  $("#btnOpenLogsModal").onclick = () => {
    $("#deviceLogsModal").classList.remove("hidden");
  };
  $("#btnCloseLogsModal").onclick = () => {
    $("#deviceLogsModal").classList.add("hidden");
  };

  async function fetchLog(type, name) {
    $("#deviceLogsModal").classList.add("hidden");
    askDeviceConfirmation(
      `Подтверждение снятия ${name}`,
      `Выполняется выгрузка журнала <b>${name}</b> через отладочный интерфейс ADB.<br><br>
      Во время выгрузки устройство может кратковременно зависнуть или <b style="color:var(--pink)">перезагрузиться</b>.<br><br>
      <span style="color:#ff8fab">Не трогайте телефон и кабель до завершения передачи данных!</span>`,
      async () => {
        toast(`Чтение лога: ${name}...`, "pink");
        try {
          const logText = await invoke("read_device_logs", { logType: type });
          $("#devLogView").textContent = logText;
          
          // Переключаем на вкладку логов
          $('#deviceSubTabs [data-tab="logs"]').click();

          // Подсвечиваем активную кнопку лога
          $("#btnLogcatTab").className = type === "logcat" ? "btn sm primary" : "btn sm ghost";
          $("#btnDmesgTab").className = type === "dmesg" ? "btn sm primary" : "btn sm ghost";
          $("#btnRecLogTab").className = type === "recovery" ? "btn sm primary" : "btn sm ghost";

          toast(`Лог ${name} успешно загружен!`);
        } catch (err) {
          toast("Ошибка чтения лога: " + err);
        }
      }
    );
  }

  $("#btnTakeLogcat").onclick = () => fetchLog("logcat", "Logcat (Android OS)");
  $("#btnTakeDmesg").onclick = () => fetchLog("dmesg", "Dmesg (Ядро)");
  $("#btnTakeRecovery").onclick = () => fetchLog("recovery", "Recovery Log");

  $("#btnLogcatTab").onclick = () => fetchLog("logcat", "Logcat");
  $("#btnDmesgTab").onclick = () => fetchLog("dmesg", "Dmesg");
  $("#btnRecLogTab").onclick = () => fetchLog("recovery", "Recovery Log");

  $("#btnCopyLogText").onclick = () => {
    const text = $("#devLogView").textContent;
    if (text) {
      navigator.clipboard.writeText(text);
      toast("Лог скопирован в буфер обмена!", "pink");
    }
  };

  function renderDevicePartitions(parts) {
    const tbody = $("#devPartsTbody");
    tbody.innerHTML = "";
    if (!parts || parts.length === 0) {
      tbody.innerHTML = `<tr><td colspan="4" style="text-align:center;padding:20px;color:var(--muted)">Разделы не обнаружены (в fastboot режиме доступен ограниченный список getvar).</td></tr>`;
      return;
    }
    parts.forEach(p => {
      const tr = document.createElement("tr");
      tr.innerHTML = `
        <td><b>${p.mount_point || p.name}</b></td>
        <td><span class="badge ${p.filesystem.includes('erofs') ? 'erofs' : (p.filesystem.includes('ext4') ? 'ext4' : 'raw')}">${p.filesystem || 'raw'}</span></td>
        <td>${p.size_human}</td>
        <td style="color:var(--muted);font-family:Consolas,monospace;font-size:12px">${p.block_device}</td>
      `;
      tbody.appendChild(tr);
    });
  }

  function renderDeviceProps(props, query) {
    const tbody = $("#devPropsTbody");
    tbody.innerHTML = "";
    const q = (query || "").toLowerCase();

    let count = 0;
    for (const [k, v] of Object.entries(props)) {
      if (q && !k.toLowerCase().includes(q) && !v.toLowerCase().includes(q)) continue;
      count++;
      const tr = document.createElement("tr");
      tr.innerHTML = `
        <td style="font-family:Consolas,monospace;font-size:12px;color:var(--sky)">${k}</td>
        <td style="font-family:Consolas,monospace;font-size:12px;user-select:text;word-break:break-all">${v}</td>
      `;
      tbody.appendChild(tr);
      if (count > 800) {
        const trMore = document.createElement("tr");
        trMore.innerHTML = `<td colspan="2" style="text-align:center;color:var(--muted)">Показано 800 записей. Уточните поиск для остальных.</td>`;
        tbody.appendChild(trMore);
        break;
      }
    }

    if (count === 0) {
      tbody.innerHTML = `<tr><td colspan="2" style="text-align:center;padding:20px;color:var(--muted)">Свойства getprop не найдены или не соответствуют фильтру.</td></tr>`;
    }
  }

  function renderFastbootVars(vars, query) {
    const tbody = $("#devFbVarsTbody");
    tbody.innerHTML = "";
    const q = (query || "").toLowerCase();

    let count = 0;
    for (const [k, v] of Object.entries(vars)) {
      if (q && !k.toLowerCase().includes(q) && !v.toLowerCase().includes(q)) continue;
      count++;
      const tr = document.createElement("tr");
      tr.innerHTML = `
        <td style="font-family:Consolas,monospace;font-size:12px;color:var(--pink)">${k}</td>
        <td style="font-family:Consolas,monospace;font-size:12px;user-select:text;word-break:break-all">${v}</td>
      `;
      tbody.appendChild(tr);
    }

    if (count === 0) {
      tbody.innerHTML = `<tr><td colspan="2" style="text-align:center;padding:20px;color:var(--muted)">Переменные fastboot не найдены (опрос bootloader не выполнялся или не дал результата).</td></tr>`;
    }
  }

  $("#inputPropFilter").addEventListener("input", (e) => {
    if (!currentDevInfo) return;
    renderDeviceProps(currentDevInfo.build_props || {}, e.target.value.trim());
  });

  $("#inputFbVarFilter").addEventListener("input", (e) => {
    if (!currentDevInfo) return;
    renderFastbootVars(currentDevInfo.fastboot_vars || {}, e.target.value.trim());
  });

  // Переключение вкладок внутри экрана устройства
  $$("#deviceSubTabs .f-pill").forEach(btn => {
    btn.onclick = () => {
      $$("#deviceSubTabs .f-pill").forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      const tab = btn.dataset.tab;
      $("#devTabParts").classList.toggle("hidden", tab !== "parts");
      $("#devTabProps").classList.toggle("hidden", tab !== "props");
      $("#devTabSettings").classList.toggle("hidden", tab !== "settings");
      $("#devTabDiagnostics").classList.toggle("hidden", tab !== "diagnostics");
      $("#devTabFastboot").classList.toggle("hidden", tab !== "fastboot");
      $("#devTabFstab").classList.toggle("hidden", tab !== "fstab");
      $("#devTabLogs").classList.toggle("hidden", tab !== "logs");
    };
  });

  /* ---------- Модификации, Патчи & Бинарники ---------- */
  function switchPatchTab(tab) {
    $$("#patchToolsTabs .f-pill").forEach(b => b.classList.toggle("active", b.dataset.ptab === tab));
    $("#pTabBoot").classList.toggle("hidden", tab !== "boot");
    $("#pTabRoot").classList.toggle("hidden", tab !== "root");
    $("#pTabVbmeta").classList.toggle("hidden", tab !== "vbmeta");
    $("#pTabDtb").classList.toggle("hidden", tab !== "dtb");
    $("#pTabLk").classList.toggle("hidden", tab !== "lk");
    $("#pTabElf").classList.toggle("hidden", tab !== "elf");
    $("#pTabProps").classList.toggle("hidden", tab !== "props");
  }

  $$("#patchToolsTabs .f-pill").forEach(btn => {
    btn.onclick = () => switchPatchTab(btn.dataset.ptab);
  });

  // --- 1. Boot & Kernel Патчер ---
  async function inspectBootFolder(name) {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    try {
      const d = await invoke("boot_inspect_extracted", { project: currentProject, targetName: name });
      $("#bootInspectDetails").classList.remove("hidden");
      $("#bootInfoOs").textContent = `${d.os_version || 'не указана'} (патч: ${d.os_patch_level || '—'})`;
      
      const comps = [];
      if (d.has_kernel) comps.push("kernel");
      if (d.has_ramdisk) comps.push("ramdisk");
      if (d.has_header) comps.push("header");
      if (d.has_dtb) comps.push("dtb");
      $("#bootInfoComps").textContent = comps.join(", ") || "не найдены";
      
      $("#bootInfoCmdline").textContent = d.cmdline || "(пусто)";
      if (d.cmdline) {
        $("#inputCustomCmdline").value = d.cmdline;
      }

      $("#bootInfoFstabs").textContent = d.fstab_files.length > 0 
        ? d.fstab_files.map(f => f.split(/[\\/]/).pop()).join(", ") 
        : "нет";

      $("#bootInfoProps").textContent = d.prop_files.length > 0 
        ? d.prop_files.map(f => f.split(/[\\/]/).pop()).join(", ") 
        : "нет";

      toast(`Проинспектирован extracted/${name}`, "sky");
    } catch (e) {
      toast("Ошибка инспекции boot: " + e);
    }
  }

  $("#btnInspectBootFolder").onclick = () => {
    const name = $("#inputBootExtractedName").value.trim() || "boot";
    inspectBootFolder(name);
  };

  $("#btnApplyBootPatches").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const targetName = $("#inputBootExtractedName").value.trim() || "boot";
    const customCmd = $("#inputCustomCmdline").value.trim();

    const options = {
      selinux_permissive: $("#chkPatchSelinux").checked,
      disable_dm_verity: $("#chkPatchVerity").checked,
      disable_force_encrypt: $("#chkPatchForceEncrypt").checked,
      enable_adb_root: $("#chkPatchAdbRoot").checked,
      custom_cmdline: customCmd ? customCmd : null
    };

    try {
      const logs = await invoke("boot_apply_patches", { project: currentProject, targetName, options });
      const logBox = $("#bootPatchLog");
      logBox.classList.remove("hidden");
      logBox.textContent = "✓ Результаты патчинга:\n" + logs.map(l => "• " + l).join("\n");
      toast("Патчи успешно применены к extracted/" + targetName, "sky");
      inspectBootFolder(targetName);
    } catch (e) {
      toast("Ошибка применения патчей: " + e);
    }
  };

  $("#btnApplyKernelHex").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const targetName = $("#inputBootExtractedName").value.trim() || "boot";
    const patternHex = $("#inputKernelHexFind").value.trim();
    const replaceHex = $("#inputKernelHexReplace").value.trim();
    if (!patternHex || !replaceHex) {
      toast("Укажите шаблон поиска и замены в HEX!");
      return;
    }
    try {
      const res = await invoke("boot_kernel_hex_patch", { project: currentProject, targetName, patternHex, replaceHex });
      toast(res, "sky");
    } catch (e) {
      toast("Ошибка HEX-патча ядра: " + e);
    }
  };

  $("#btnPresetSkipInitramfs").onclick = () => {
    $("#inputKernelHexFind").value = "73 6B 69 70 5F 69 6E 69 74 72 61 6D 66 73";
    $("#inputKernelHexReplace").value = "77 61 6E 74 5F 69 6E 69 74 72 61 6D 66 73";
    toast("Загружен пресет: skip_initramfs → want_initramfs", "pink");
  };

  $("#btnPresetEnforcing").onclick = () => {
    $("#inputKernelHexFind").value = "65 6E 66 6F 72 63 69 6E 67 3D 31";
    $("#inputKernelHexReplace").value = "65 6E 66 6F 72 63 69 6E 67 3D 30";
    toast("Загружен пресет: enforcing=1 → enforcing=0", "pink");
  };

  $("#btnSelectBootFile").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите или создайте проект!", "pink");
      go("projects");
      return;
    }
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'Boot / Recovery образы', extensions: ['img', 'bin'] }]
      });
      if (file) {
        try {
          await invoke("unpack_boot", { project: currentProject, imagePath: file });
          toast("Запущена распаковка Boot/Recovery ядра через MagiskBoot...", "pink");
        } catch (e) { toast("Ошибка: " + e); }
      }
    }
  };

  $("#btnRepackBoot").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const name = prompt("Введите имя папки из extracted/ (например boot или recovery):", $("#inputBootExtractedName").value || "boot");
    if (name) {
      try {
        await invoke("repack_boot", { project: currentProject, targetName: name });
        toast(`Запущена обратная сборка ${name}...`, "pink");
      } catch (e) { toast("Ошибка: " + e); }
    }
  };

  // --- Вкладка: Root Инжектор ---
  $("#btnCheckRootStatus").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const folder = $("#inputRootBootFolder").value.trim() || "boot";
    try {
      const st = await invoke("root_inspect", { project: currentProject, targetFolder: folder });
      const box = $("#rootStatusBox");
      const text = $("#rootStatusText");
      box.classList.remove("hidden");
      
      const parts = [];
      parts.push(`<b>Magisk:</b> ${st.has_magisk ? '<span style="color:var(--sky)">Обнаружен</span>' : '<span style="color:var(--muted)">Нет</span>'}`);
      parts.push(`<b>KernelSU:</b> ${st.has_kernelsu ? '<span style="color:var(--sky)">Обнаружен</span>' : '<span style="color:var(--muted)">Нет</span>'}`);
      parts.push(`<b>APatch:</b> ${st.has_apatch ? '<span style="color:var(--sky)">Обнаружен</span>' : '<span style="color:var(--muted)">Нет</span>'}`);
      parts.push(`<b>Overlay.d:</b> ${st.ramdisk_has_overlay ? 'Присутствует' : 'Отсутствует'}`);
      
      if (st.details.length > 0) {
        parts.push(`<br><small>${st.details.join("<br>")}</small>`);
      }
      text.innerHTML = parts.join(" &nbsp;|&nbsp; ");
      toast("Статус Root-компонентов проверен", "sky");
    } catch (e) {
      toast("Ошибка проверки Root: " + e);
    }
  };

  $("#btnInjectRoot").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const folder = $("#inputRootBootFolder").value.trim() || "boot";
    const selectedRadio = document.querySelector('input[name="root_type"]:checked');
    const rootType = selectedRadio ? selectedRadio.value : "magisk";

    try {
      const msg = await invoke("root_inject_hooks", {
        req: {
          project: currentProject,
          target_boot_folder: folder,
          root_type: rootType,
          magisk_apk_path: null
        }
      });
      toast(msg, "sky");
      $("#btnCheckRootStatus").click();
    } catch (e) {
      toast("Ошибка внедрения Root: " + e);
    }
  };

  // --- Вкладка: VBMeta & AVB ---
  let selectedVbmetaPath = null;

  $("#btnSelectVbmeta").onclick = async () => {
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'VBMeta образы', extensions: ['img', 'bin'] }]
      });
      if (file) {
        selectedVbmetaPath = file;
        try {
          const info = await invoke("vbmeta_inspect", { filePath: file });
          $("#vbmetaInfoCard").classList.remove("hidden");
          $("#vbmetaFile").textContent = info.file_name;
          $("#vbmetaLibVer").textContent = info.required_libavb_version;
          $("#vbmetaAlgorithm").textContent = info.algorithm_name;
          $("#vbmetaFlagsHex").textContent = `0x${info.flags.toString(16).toUpperCase()}`;
          $("#vbmetaStatusVerity").innerHTML = info.verity_disabled ? '<span style="color:var(--sky)">Отключен (Disabled)</span>' : '<span style="color:var(--pink)">Включен (Enforcing)</span>';
          $("#vbmetaStatusVerification").innerHTML = info.verification_disabled ? '<span style="color:var(--sky)">Отключена (Disabled)</span>' : '<span style="color:var(--pink)">Включена (Active)</span>';
          $("#vbmetaParts").textContent = info.partitions_in_vbmeta.join(", ") || "не найдены";
          toast("VBMeta успешно прочитан", "sky");
        } catch (e) {
          toast("Ошибка чтения VBMeta: " + e);
        }
      }
    }
  };

  $("#btnPatchVbmetaFlags").onclick = async () => {
    if (!selectedVbmetaPath) {
      toast("Сначала выберите файл vbmeta.img!", "pink");
      return;
    }
    try {
      const msg = await invoke("vbmeta_patch_disable_flags", { filePath: selectedVbmetaPath });
      toast(msg, "sky");
      const info = await invoke("vbmeta_inspect", { filePath: selectedVbmetaPath });
      $("#vbmetaFlagsHex").textContent = `0x${info.flags.toString(16).toUpperCase()}`;
      $("#vbmetaStatusVerity").innerHTML = '<span style="color:var(--sky)">Отключен (Disabled)</span>';
      $("#vbmetaStatusVerification").innerHTML = '<span style="color:var(--sky)">Отключена (Disabled)</span>';
    } catch (e) {
      toast("Ошибка патчинга VBMeta: " + e);
    }
  };

  $("#btnGenerateBlankVbmeta").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const outName = prompt("Введите имя сохраняемого файла:", "vbmeta_disabled.img");
    if (!outName) return;

    if (T && T.dialog) {
      const savePath = await T.dialog.save({
        defaultPath: outName,
        filters: [{ name: 'Образ VBMeta', extensions: ['img'] }]
      });
      if (savePath) {
        try {
          const msg = await invoke("vbmeta_generate_blank", { outPath: savePath });
          toast(msg, "sky");
        } catch (e) {
          toast("Ошибка генерации VBMeta: " + e);
        }
      }
    }
  };

  // --- Вкладка: DTB / DTS Studio ---
  let selectedDtbPath = null;

  $("#btnSelectDtbFile").onclick = async () => {
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'Device Tree (dtb, dtbo, img)', extensions: ['dtb', 'dtbo', 'img', '*'] }]
      });
      if (file) {
        selectedDtbPath = file;
        try {
          const d = await invoke("dtb_inspect", { filePath: file });
          $("#dtbDetailsCard").classList.remove("hidden");
          $("#dtbModel").textContent = d.model;
          $("#dtbType").textContent = d.is_dtbo ? "DTBO (Overlay)" : "DTB (Base FDT)";
          $("#dtbSize").textContent = formatBytes(d.total_size || d.file_size);
          $("#dtbBootargs").textContent = d.bootargs || "(нет явных bootargs)";

          const compBox = $("#dtbCompatiblePills");
          compBox.innerHTML = "";
          d.compatible.forEach(c => {
            const span = document.createElement("span");
            span.className = "pill";
            span.textContent = c;
            compBox.appendChild(span);
          });

          const nodeBox = $("#dtbNodesPills");
          nodeBox.innerHTML = "";
          d.found_nodes.forEach(n => {
            const span = document.createElement("span");
            span.className = "pill";
            span.style.background = "rgba(142,197,255,0.08)";
            span.textContent = n;
            nodeBox.appendChild(span);
          });

          toast("Device Tree успешно проанализирован", "sky");
        } catch (e) {
          toast("Ошибка анализа DTB: " + e);
        }
      }
    }
  };

  $("#btnDecompileDtb").onclick = async () => {
    if (!selectedDtbPath) {
      toast("Сначала выберите файл DTB!", "pink");
      return;
    }
    if (T && T.dialog) {
      const saveDts = await T.dialog.save({
        defaultPath: "decompiled.dts",
        filters: [{ name: 'Device Tree Source', extensions: ['dts'] }]
      });
      if (saveDts) {
        try {
          const msg = await invoke("dtb_decompile_to_dts", { dtbPath: selectedDtbPath, outDtsPath: saveDts });
          toast(msg, "sky");
        } catch (e) {
          toast("Ошибка декомпиляции: " + e);
        }
      }
    }
  };

  $("#btnCompileDts").onclick = async () => {
    if (T && T.dialog) {
      const dtsFile = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'Device Tree Source', extensions: ['dts'] }]
      });
      if (dtsFile) {
        const saveDtb = await T.dialog.save({
          defaultPath: "compiled.dtb",
          filters: [{ name: 'Compiled Device Tree', extensions: ['dtb', 'dtbo'] }]
        });
        if (saveDtb) {
          try {
            const msg = await invoke("dtb_compile_from_dts", { dtsPath: dtsFile, outDtbPath: saveDtb });
            toast(msg, "sky");
          } catch (e) {
            toast("Ошибка компиляции DTS: " + e);
          }
        }
      }
    }
  };

  // --- 2. LK & Bootloader Анализатор ---
  let lastLkResult = null;

  function renderLkResults(res) {
    lastLkResult = res;
    $("#lkSummaryBar").classList.remove("hidden");
    $("#lkResultsWrap").classList.remove("hidden");

    $("#lkPlatform").textContent = res.platform || "Generic";
    $("#lkSize").textContent = formatBytes(res.file_size);
    $("#lkOemCount").textContent = res.oem_commands.length;
    $("#lkFbCount").textContent = res.fastboot_commands.length;
    $("#lkPartsCount").textContent = res.partitions.length;

    const tbody = $("#lkOemTbody");
    tbody.innerHTML = "";
    if (res.oem_commands.length === 0) {
      tbody.innerHTML = `<tr><td colspan="3" style="text-align:center;padding:16px;color:var(--muted)">В этом файле не найдены явные fastboot oem команды.</td></tr>`;
    } else {
      res.oem_commands.forEach(cmd => {
        const tr = document.createElement("tr");
        tr.innerHTML = `
          <td><b style="color:var(--pink)">fastboot ${cmd.name}</b></td>
          <td><code class="code-sm">0x${cmd.offset.toString(16).toUpperCase()}</code></td>
          <td><small class="code-sm" style="color:var(--muted)">${escapeHtml(cmd.context)}</small></td>
        `;
        tbody.appendChild(tr);
      });
    }

    // Разделы LK
    const partsPills = $("#lkPartsPills");
    partsPills.innerHTML = "";
    if (res.partitions.length === 0) {
      partsPills.innerHTML = `<span class="muted" style="font-size:12px">Разделы не обнаружены</span>`;
    } else {
      res.partitions.forEach(p => {
        const span = document.createElement("span");
        span.className = "pill";
        span.textContent = p;
        partsPills.appendChild(span);
      });
    }

    // Флаги безопасности
    const secPills = $("#lkSecurityPills");
    secPills.innerHTML = "";
    if (res.security_flags.length === 0) {
      secPills.innerHTML = `<span class="muted" style="font-size:12px">Флаги безопасности не обнаружены</span>`;
    } else {
      res.security_flags.forEach(f => {
        const span = document.createElement("span");
        span.className = "pill";
        span.style.borderColor = "rgba(255,107,138,0.4)";
        span.style.color = "#ff8fab";
        span.textContent = f;
        secPills.appendChild(span);
      });
    }
  }

  async function analyzeLkPartition(partName) {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    try {
      toast(`Анализ раздела ${partName}...`, "pink");
      const res = await invoke("lk_analyze_partition", { project: currentProject, partitionName: partName });
      renderLkResults(res);
      toast(`Найдено ${res.oem_commands.length} OEM команд в ${partName}`, "sky");
    } catch (e) {
      toast("Ошибка анализа LK: " + e);
    }
  }

  $("#btnSelectLkFile").onclick = async () => {
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'Загрузчики (LK / ABL / XBL / Bootloader)', extensions: ['bin', 'img', 'elf', 'mbn'] }]
      });
      if (file) {
        try {
          toast("Анализ файла загрузчика...", "pink");
          const res = await invoke("lk_analyze_file", { filePath: file });
          renderLkResults(res);
          toast(`Анализ завершен! Найдено ${res.oem_commands.length} OEM команд`, "sky");
        } catch (e) {
          toast("Ошибка анализа: " + e);
        }
      }
    }
  };

  $("#btnAnalyzeLkFromProject").onclick = async () => {
    if (!currentProject) {
      toast("Сначала выберите проект!", "pink");
      return;
    }
    const part = prompt("Введите имя раздела загрузчика в проекте (например lk, abl, xbl, bootloader):", "lk");
    if (part) {
      analyzeLkPartition(part);
    }
  };

  $("#btnCopyOemCmds").onclick = () => {
    if (!lastLkResult || lastLkResult.oem_commands.length === 0) {
      toast("Сначала выполните анализ загрузчика с OEM командами!");
      return;
    }
    const lines = lastLkResult.oem_commands.map(c => `fastboot ${c.name}`).join("\n");
    navigator.clipboard.writeText(lines).then(() => {
      toast(`Скопировано ${lastLkResult.oem_commands.length} OEM команд в буфер!`, "sky");
    }).catch(e => toast("Ошибка буфера: " + e));
  };

  $("#inputLkOemFilter").oninput = (e) => {
    if (!lastLkResult) return;
    const q = e.target.value.toLowerCase().trim();
    const tbody = $("#lkOemTbody");
    tbody.innerHTML = "";
    const filtered = lastLkResult.oem_commands.filter(c => !q || c.name.toLowerCase().includes(q) || c.context.toLowerCase().includes(q));
    if (filtered.length === 0) {
      tbody.innerHTML = `<tr><td colspan="3" style="text-align:center;padding:16px;color:var(--muted)">Команды не найдены по фильтру.</td></tr>`;
      return;
    }
    filtered.forEach(cmd => {
      const tr = document.createElement("tr");
      tr.innerHTML = `
        <td><b style="color:var(--pink)">fastboot ${cmd.name}</b></td>
        <td><code class="code-sm">0x${cmd.offset.toString(16).toUpperCase()}</code></td>
        <td><small class="code-sm" style="color:var(--muted)">${escapeHtml(cmd.context)}</small></td>
      `;
      tbody.appendChild(tr);
    });
  };

  // --- 3. ELF & Binary Патчер ---
  let currentElfData = null;
  let currentElfPath = null;

  function renderElfTable(filterText = "") {
    if (!currentElfData) return;
    const tbody = $("#elfSymbolsTbody");
    tbody.innerHTML = "";

    const filter = filterText.toLowerCase();
    const list = currentElfData.functions.filter(f => {
      if (!filter) return true;
      return f.name.toLowerCase().includes(filter) || f.section.toLowerCase().includes(filter);
    });

    if (list.length === 0) {
      tbody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:16px;color:var(--muted)">Функции не найдены по фильтру.</td></tr>`;
      return;
    }

    list.slice(0, 300).forEach(fn => {
      const tr = document.createElement("tr");
      tr.style.cursor = "pointer";
      const hasOffset = fn.file_offset !== null && fn.file_offset !== undefined;
      const offHex = hasOffset ? `0x${fn.file_offset.toString(16).toUpperCase()}` : "IMPORT";

      tr.innerHTML = `
        <td><b style="color:${hasOffset ? 'var(--text)' : 'var(--muted)'}">${escapeHtml(fn.name)}</b></td>
        <td><code class="code-sm">0x${fn.address.toString(16).toUpperCase()}</code></td>
        <td><code class="code-sm" style="color:${hasOffset ? 'var(--sky)' : '#ff8fab'}">${offHex}</code></td>
        <td><span class="badge ${fn.binding === 'GLOBAL' ? 'ext4' : 'raw'}">${fn.binding}</span></td>
        <td>
          ${hasOffset ? `<button class="btn sm ghost btn-pick-func">Выбрать</button>` : `<small class="muted">Внешний</small>`}
        </td>
      `;

      tr.onclick = () => {
        if (hasOffset) {
          $("#inputTargetSymbol").value = fn.name;
          $("#inputTargetOffset").value = offHex;
          $$("#elfSymbolsTbody tr").forEach(r => r.style.background = "");
          tr.style.background = "rgba(142,197,255,0.08)";
        }
      };

      tbody.appendChild(tr);
    });
  }

  async function analyzeElf(filePath) {
    try {
      toast("Анализ ELF бинарника...", "pink");
      const res = await invoke("elf_analyze", { filePath });
      currentElfData = res;
      currentElfPath = filePath;

      $("#elfMetaBar").classList.remove("hidden");
      $("#elfWorkspace").classList.remove("hidden");

      $("#elfFileName").textContent = res.file_name;
      $("#elfArch").textContent = res.architecture;
      $("#elfType").textContent = res.elf_type;
      $("#elfSymCount").textContent = res.total_symbols;

      const bakPill = $("#elfBackupStatus");
      if (res.has_backup) {
        bakPill.innerHTML = `<i></i>Бэкап (.bak) есть`;
        bakPill.style.color = "var(--sky)";
      } else {
        bakPill.innerHTML = `<i></i>Бэкап: будет создан`;
        bakPill.style.color = "var(--muted)";
      }

      renderElfTable();
      toast(`Бинарник ${res.file_name} загружен: ${res.total_symbols} функций`, "sky");
    } catch (e) {
      toast("Ошибка анализа ELF: " + e);
    }
  }

  $("#btnSelectElfFile").onclick = async () => {
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'ELF бинарники (*.so, bin, exe)', extensions: ['so', 'bin', 'elf', '*'] }]
      });
      if (file) {
        analyzeElf(file);
      }
    }
  };

  $("#inputElfFilter").oninput = (e) => {
    renderElfTable(e.target.value);
  };

  $("#btnFilterRootChecks").onclick = () => {
    const input = $("#inputElfFilter");
    input.value = "check|verify|root|security|signature|tamper";
    // Фильтр по любому из ключевых слов
    if (!currentElfData) return;
    const keywords = ["check", "verify", "root", "security", "signature", "tamper", "license", "debug", "su"];
    const tbody = $("#elfSymbolsTbody");
    tbody.innerHTML = "";

    const list = currentElfData.functions.filter(f => {
      const low = f.name.toLowerCase();
      return keywords.some(k => low.includes(k));
    });

    if (list.length === 0) {
      tbody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:16px;color:var(--muted)">Функций с проверками безопасности не найдено.</td></tr>`;
      return;
    }

    list.forEach(fn => {
      const tr = document.createElement("tr");
      const hasOffset = fn.file_offset !== null && fn.file_offset !== undefined;
      const offHex = hasOffset ? `0x${fn.file_offset.toString(16).toUpperCase()}` : "IMPORT";

      tr.innerHTML = `
        <td><b style="color:${hasOffset ? 'var(--pink)' : 'var(--muted)'}">${icon('shield', 13)} ${escapeHtml(fn.name)}</b></td>
        <td><code class="code-sm">0x${fn.address.toString(16).toUpperCase()}</code></td>
        <td><code class="code-sm" style="color:${hasOffset ? 'var(--sky)' : '#ff8fab'}">${offHex}</code></td>
        <td><span class="badge ${fn.binding === 'GLOBAL' ? 'ext4' : 'raw'}">${fn.binding}</span></td>
        <td>
          ${hasOffset ? `<button class="btn sm ghost btn-pick-func">Выбрать</button>` : `<small class="muted">Внешний</small>`}
        </td>
      `;

      tr.onclick = () => {
        if (hasOffset) {
          $("#inputTargetSymbol").value = fn.name;
          $("#inputTargetOffset").value = offHex;
        }
      };

      tbody.appendChild(tr);
    });
  };

  $("#selStubType").onchange = (e) => {
    const v = e.target.value;
    $("#customHexRow").classList.toggle("hidden", v !== "custom_hex");
    $("#nopCountRow").classList.toggle("hidden", v !== "nop");
  };

  $("#btnApplyElfPatch").onclick = async () => {
    if (!currentElfPath) {
      toast("Сначала выберите ELF бинарник!", "pink");
      return;
    }

    const symName = $("#inputTargetSymbol").value.trim() || null;
    const offStr = $("#inputTargetOffset").value.trim();
    let fileOffset = null;
    if (offStr) {
      fileOffset = parseInt(offStr, offStr.startsWith("0x") || offStr.startsWith("0X") ? 16 : 10);
      if (isNaN(fileOffset)) fileOffset = null;
    }

    if (!symName && fileOffset === null) {
      toast("Укажите функцию или смещение для патча!");
      return;
    }

    const stubType = $("#selStubType").value;
    const customHex = stubType === "custom_hex" ? $("#inputCustomHex").value.trim() : null;
    const nopCount = stubType === "nop" ? parseInt($("#inputNopCount").value, 10) : null;

    const req = {
      file_path: currentElfPath,
      symbol_name: symName,
      file_offset: fileOffset,
      stub_type: stubType,
      custom_hex: customHex,
      nop_count: nopCount
    };

    try {
      const res = await invoke("elf_apply_patch", { req });
      const statusLbl = $("#patchStatusLabel");
      statusLbl.classList.remove("hidden");
      statusLbl.innerHTML = `✓ Патч применен! Смещение <b>0x${res.offset.toString(16).toUpperCase()}</b>: <code>${res.old_bytes_hex}</code> → <code style="color:var(--sky)">${res.new_bytes_hex}</code>`;
      toast("Функция успешно пропатчена заглушкой!", "sky");
      analyzeElf(currentElfPath);
    } catch (e) {
      toast("Ошибка патчинга: " + e);
    }
  };

  $("#btnRestoreElfBackup").onclick = async () => {
    if (!currentElfPath) {
      toast("Файл не выбран!");
      return;
    }
    if (!confirm(`Восстановить ${currentElfPath} из резервной копии (.bak)?`)) return;
    try {
      const msg = await invoke("elf_restore_backup", { filePath: currentElfPath });
      toast(msg, "sky");
      analyzeElf(currentElfPath);
    } catch (e) {
      toast("Ошибка восстановления: " + e);
    }
  };

  $("#btnApplyStrPatch").onclick = async () => {
    if (!currentElfPath) {
      toast("Сначала выберите ELF бинарник!", "pink");
      return;
    }
    const oldStr = $("#inputStrOld").value.trim();
    const newStr = $("#inputStrNew").value.trim();
    if (!oldStr || !newStr) {
      toast("Введите исходную и новую строку!");
      return;
    }
    try {
      const msg = await invoke("elf_patch_string", { filePath: currentElfPath, oldStr, newStr });
      const statusBox = $("#strPatchStatus");
      statusBox.classList.remove("hidden");
      statusBox.textContent = `✓ ${msg}`;
      toast(msg, "sky");
      analyzeElf(currentElfPath);
    } catch (e) {
      toast("Ошибка замены строки: " + e);
    }
  };

  // --- Вкладка: build.prop Твики и Спуфинг ---
  let selectedPropPath = null;
  let currentPropData = null;

  async function loadPropPresets() {
    try {
      const presets = await invoke("prop_list_presets");
      const grid = $("#propPresetsGrid");
      grid.innerHTML = "";
      presets.forEach(p => {
        const card = document.createElement("article");
        card.className = "card";
        card.style.cursor = "pointer";
        card.innerHTML = `
          <div class="ic">${icon('zap')}</div>
          <h3>${escapeHtml(p.name)}</h3>
          <p>${escapeHtml(p.description)}</p>
          <div style="margin-top:10px">
            <button class="btn sm primary btn-apply-preset">Применить пресет</button>
          </div>
        `;
        card.querySelector(".btn-apply-preset").onclick = async (e) => {
          e.stopPropagation();
          if (!selectedPropPath) {
            toast("Сначала выберите файл .prop!", "pink");
            return;
          }
          try {
            const msg = await invoke("prop_apply_tweaks", { filePath: selectedPropPath, tweaks: p.props });
            toast(msg, "sky");
            loadPropFile(selectedPropPath);
          } catch (err) {
            toast("Ошибка применения твиков: " + err);
          }
        };
        grid.appendChild(card);
      });
    } catch (e) {
      console.error(e);
    }
  }

  function renderPropTable(filterText = "") {
    if (!currentPropData) return;
    const tbody = $("#propEntriesTbody");
    tbody.innerHTML = "";
    const filter = filterText.toLowerCase();

    const filtered = currentPropData.props.filter(p => {
      if (!filter) return true;
      return p.key.toLowerCase().includes(filter) || p.value.toLowerCase().includes(filter);
    });

    if (filtered.length === 0) {
      tbody.innerHTML = `<tr><td colspan="3" style="text-align:center;padding:16px;color:var(--muted)">Свойства не найдены.</td></tr>`;
      return;
    }

    filtered.slice(0, 300).forEach(p => {
      const tr = document.createElement("tr");
      tr.innerHTML = `
        <td><b>${escapeHtml(p.key)}</b></td>
        <td><code class="code-sm" style="color:var(--sky)">${escapeHtml(p.value)}</code></td>
        <td><small class="muted">${p.line_num}</small></td>
      `;
      tbody.appendChild(tr);
    });
  }

  async function loadPropFile(filePath) {
    try {
      const data = await invoke("prop_read_file", { filePath });
      selectedPropPath = filePath;
      currentPropData = data;

      $("#propFileBar").classList.remove("hidden");
      $("#propWorkspace").classList.remove("hidden");

      $("#propFileName").textContent = filePath.split(/[\\/]/).pop();
      $("#propTotalCount").textContent = data.total_props;
      $("#propDuplPill").textContent = `Дубликатов: ${data.duplicates.length}`;

      await loadPropPresets();
      renderPropTable();
      toast(`Файл свойств загружен (${data.total_props} записей)`, "sky");
    } catch (e) {
      toast("Ошибка чтения .prop файла: " + e);
    }
  }

  $("#btnSelectPropFile").onclick = async () => {
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'Файлы свойств Android', extensions: ['prop', 'txt', '*'] }]
      });
      if (file) {
        loadPropFile(file);
      }
    }
  };

  $("#inputPropSearch").oninput = (e) => {
    renderPropTable(e.target.value);
  };

  $("#btnRestorePropBackup").onclick = async () => {
    if (!selectedPropPath) {
      toast("Сначала выберите файл .prop!", "pink");
      return;
    }
    const bak = selectedPropPath + ".bak";
    if (!confirm(`Восстановить из резервной копии (${bak})?`)) return;
    try {
      await invoke("fs_copy_entry", { src: bak, dst: selectedPropPath });
      toast("Файл свойств успешно восстановлен из .bak", "sky");
      loadPropFile(selectedPropPath);
    } catch (e) {
      toast("Ошибка восстановления: " + e);
    }
  };

  /* ---------- инструменты ---------- */
  async function refreshTools() {
    const list = $("#toolsList");
    try {
      const items = await invoke("tools_list");
      list.innerHTML = "";
      
      const filtered = items.filter(it => currentCategory === "all" || it.category === currentCategory);

      if (filtered.length === 0) {
        list.innerHTML = `<div class="empty-state" style="grid-column: 1 / -1">В этой категории пока нет инструментов.</div>`;
        return;
      }

      filtered.forEach(item => {
        const card = document.createElement("div");
        card.className = "tool-card";
        const latestVer = toolsLatestMap[item.id];
        const isUpToDate = item.installed && latestVer && item.installed === latestVer;

        card.innerHTML = `
          <div class="tool-title-row">
            <div>
              <span class="tool-category">${item.category || 'Utility'}</span>
              <h3>${item.name}</h3>
            </div>
            <span class="badge ${item.ok ? 'ext4' : 'raw'}">${item.ok ? 'Готов к работе' : 'Не установлен'}</span>
          </div>
          <p class="tool-desc">${item.desc}</p>
          <div class="tool-meta">
            <div class="tool-meta-row"><span>Установленная версия:</span> <b>${item.installed || '—'}</b></div>
            <div class="tool-meta-row"><span>Доступная версия:</span> <b>${latestVer || '—'}</b></div>
            <div class="tool-meta-row"><span>Бинарники:</span> <span>${item.bins.join(', ')}</span></div>
          </div>
          <div class="tool-actions">
            ${item.installed ? `<button class="btn sm danger btn-remove-tool">Удалить</button>` : ''}
            <button class="btn sm primary btn-install-tool">
              ${item.installed ? (isUpToDate ? 'Переустановить' : 'Обновить') : 'Скачать и установить'}
            </button>
          </div>
        `;

        card.querySelector(".btn-install-tool").onclick = async () => {
          try {
            await invoke("tool_install", { id: item.id });
            toast(`Загрузка инструмента ${item.name}...`, "pink");
          } catch (e) { toast("Ошибка: " + e); }
        };

        const bRem = card.querySelector(".btn-remove-tool");
        if (bRem) {
          bRem.onclick = async () => {
            if (confirm(`Удалить инструмент «${item.name}»?`)) {
              try {
                await invoke("tool_remove", { id: item.id });
                toast(`Инструмент ${item.name} удален`);
                refreshTools();
              } catch (e) { toast("Ошибка: " + e); }
            }
          };
        }

        list.appendChild(card);
      });
    } catch (e) {
      list.innerHTML = `<div class="empty-state">Ошибка загрузки инструментов: ${e}</div>`;
    }
  }

  // Фильтры категорий инструментов
  $$("#toolCategoryFilters .f-pill").forEach(btn => {
    btn.onclick = () => {
      $$("#toolCategoryFilters .f-pill").forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      currentCategory = btn.dataset.cat;
      refreshTools();
    };
  });

  $("#btnCheckToolsUpdates").onclick = async () => {
    if (isCheckingUpdates) {
      toast("Проверка уже выполняется...");
      return;
    }
    isCheckingUpdates = true;
    const btn = $("#btnCheckToolsUpdates");
    btn.disabled = true;
    toast("Запущена поочередная проверка версий с задержкой 3 сек...");

    try {
      const items = await invoke("tools_list");
      for (let i = 0; i < items.length; i++) {
        const item = items[i];
        btn.innerHTML = `${icon('hourglass', 14)} Проверка (${i + 1}/${items.length}): ${item.name}`;

        try {
          const ver = await invoke("tool_check_latest", { id: item.id });
          toolsLatestMap[item.id] = ver;
        } catch (err) {
          toolsLatestMap[item.id] = "ошибка";
        }
        refreshTools();

        // Ожидание 3 секунды между запросами (кроме последнего элемента)
        if (i < items.length - 1) {
          await sleep(3000);
        }
      }
      toast("Проверка версий всех инструментов завершена!", "pink");
    } catch (e) {
      toast("Ошибка: " + e);
    } finally {
      isCheckingUpdates = false;
      btn.disabled = false;
      btn.innerHTML = `${icon('refresh', 14)} Проверить обновления инструментов`;
      refreshTools();
    }
  };

  /* ---------- профиль ---------- */
  let profile = null;
  function applyProfile(p) {
    profile = p;
    $("#uNick").textContent = p ? p.nickname : "Гость";
    $("#uTg").textContent = p && p.telegram ? p.telegram : "—";
    $("#avatar").textContent = p ? p.nickname[0].toUpperCase() : "?";
    $("#heroNick").textContent = p ? p.nickname : "кузнец";
  }
  function openProfile() {
    const f = $("#profileForm");
    ["nickname", "telegram", "github", "device", "about"].forEach((k) => (f[k].value = (profile && profile[k]) || ""));
    $("#profileErr").textContent = ""; $("#profileModal").classList.remove("hidden"); setTimeout(() => f.nickname.focus(), 100);
  }
  $("#profileForm").onsubmit = async (e) => {
    e.preventDefault();
    const f = e.target, p = {};
    ["nickname", "telegram", "github", "device", "about"].forEach((k) => (p[k] = f[k].value.trim()));
    if (p.telegram && !/^(@[\w\d_]{4,}|https?:\/\/t\.me\/[\w\d_]{4,}\/?)$/i.test(p.telegram)) {
      $("#profileErr").textContent = "Telegram: @username или https://t.me/username";
      f.classList.remove("shake"); void f.offsetWidth; f.classList.add("shake"); return;
    }
    try {
      await invoke("save_profile", { profile: p });
      applyProfile(p); $("#profileModal").classList.add("hidden");
      toast(`Добро пожаловать, ${p.nickname}!`, "pink");
      line(`Профиль сохранён: ${p.nickname}`, "sys");
    } catch (err) { $("#profileErr").textContent = String(err); }
  };
  $("#userChip").onclick = openProfile;
  $("#editProfile").onclick = openProfile;

  /* ---------- обновления приложения ---------- */
  async function checkUpdate() {
    const pill = $("#pillUpd"); pill.className = "pill";
    const u = await invoke("check_update");
    if (!u) return null;
    $("#appVer").textContent = "v" + u.current;
    if (u.error) { pill.classList.add("bad"); $("#updText").textContent = "Не удалось проверить: " + u.error; }
    else if (u.available) { pill.classList.add("warn"); $("#updText").textContent = `Доступна ${u.latest} (у вас v${u.current})`; }
    else { pill.classList.add("ok"); $("#updText").textContent = `Актуальная версия v${u.current}`; }
    return u;
  }
  $("#updRecheck").onclick = async () => { const u = await checkUpdate(); toast(u && u.available ? "Есть обновление!" : "Обновлений нет"); };

  /* ---------- Telegram Бот интеграция ---------- */
  let currentTgConfig = null;

  async function loadTelegramConfig() {
    try {
      const cfg = await invoke("tg_get_config");
      currentTgConfig = cfg;

      $("#tgBotToken").value = cfg.bot_token || "";
      $("#tgEnabled").checked = !!cfg.enabled;
      $("#tgPollActive").checked = !!cfg.poll_active;

      const badge = $("#tgBotStatusBadge");
      if (cfg.bot_username) {
        badge.className = "pill ok";
        badge.innerHTML = `<i></i>@${cfg.bot_username}`;
      } else if (cfg.bot_token) {
        badge.className = "pill warn";
        badge.innerHTML = `<i></i>Токен задан`;
      } else {
        badge.className = "pill bad";
        badge.innerHTML = `<i></i>Не настроен`;
      }

      // События
      if (cfg.events) {
        $("#tgEvBuildStart").checked = !!cfg.events.notify_build_start;
        $("#tgEvBuildProgress").checked = !!cfg.events.notify_build_progress;
        $("#tgEvBuildDone").checked = !!cfg.events.notify_build_done;
        $("#tgEvImport").checked = !!cfg.events.notify_import;
        $("#tgEvDevice").checked = !!cfg.events.notify_device_scan;
        $("#tgEvRelease").checked = !!cfg.events.notify_release;
      }

      renderTelegramChannels(cfg.channels || []);
    } catch (e) {
      toast("Ошибка загрузки настроек Telegram: " + e);
    }
  }

  function renderTelegramChannels(channels) {
    const tbody = $("#tgChannelsTbody");
    const countEl = $("#tgChannelsCount");
    if (countEl) countEl.textContent = channels.length;

    tbody.innerHTML = "";
    if (!channels || channels.length === 0) {
      tbody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:24px;color:var(--muted)">Нет привязанных каналов. Напишите <code>/meyakitchen apply all</code> в вашем канале или группе.</td></tr>`;
      return;
    }

    channels.forEach(c => {
      const tr = document.createElement("tr");

      const triggersHtml = (c.triggers || []).map(t => {
        const cls = t === "all" ? "badge erofs" : (t === "build" ? "badge ext4" : "badge raw");
        return `<span class="${cls}" style="margin-right:4px">${t}</span>`;
      }).join("");

      tr.innerHTML = `
        <td><b>${c.title || 'Чат'}</b></td>
        <td style="font-family:Consolas,monospace;font-size:12px;color:var(--sky)">${c.chat_id}</td>
        <td><span class="badge raw">${c.chat_type || 'channel'}</span></td>
        <td>${triggersHtml}</td>
        <td style="text-align:right">
          <button class="btn sm ghost btn-tg-test" title="Отправить тест">${icon('megaphone', 13)} Тест</button>
          <button class="btn sm ghost btn-tg-del" title="Отвязать канал">${icon('trash', 13)}</button>
        </td>
      `;

      tr.querySelector(".btn-tg-test").onclick = async () => {
        try {
          const res = await invoke("tg_send_test_message", { chatId: c.chat_id });
          toast(res, "pink");
        } catch (e) {
          toast("Ошибка отправки: " + e);
        }
      };

      tr.querySelector(".btn-tg-del").onclick = async () => {
        if (confirm(`Отвязать чат «${c.title}» (${c.chat_id})?`)) {
          try {
            await invoke("tg_remove_channel", { chatId: c.chat_id });
            toast("Канал отвязан");
            loadTelegramConfig();
          } catch (e) {
            toast("Ошибка: " + e);
          }
        }
      };

      tbody.appendChild(tr);
    });
  }

  // Проверка токена
  $("#btnTgCheckToken").onclick = async () => {
    const token = $("#tgBotToken").value.trim();
    if (!token) {
      toast("Введите токен бота!");
      return;
    }
    toast("Проверка токена через Telegram API...", "pink");
    try {
      const info = await invoke("tg_check_token", { token });
      const badge = $("#tgBotStatusBadge");
      badge.className = "pill ok";
      badge.innerHTML = `<i></i>@${info.username} (${info.first_name})`;
      toast(`Бот @${info.username} успешно проверен!`, "pink");
      loadTelegramConfig();
    } catch (e) {
      const badge = $("#tgBotStatusBadge");
      badge.className = "pill bad";
      badge.innerHTML = `<i></i>Ошибка токена`;
      toast("Ошибка токена: " + e);
    }
  };

  // Показать/скрыть токен
  $("#btnToggleTgTokenEye").onclick = () => {
    const input = $("#tgBotToken");
    if (input.type === "password") {
      input.type = "text";
      $("#btnToggleTgTokenEye").innerHTML = icon('lock', 13);
    } else {
      input.type = "password";
      $("#btnToggleTgTokenEye").innerHTML = icon('eye', 13);
    }
  };

  // Сохранение настроек
  $("#btnTgSaveConfig").onclick = async () => {
    const token = $("#tgBotToken").value.trim();
    const enabled = $("#tgEnabled").checked;
    const pollActive = $("#tgPollActive").checked;

    const events = {
      notify_build_start: $("#tgEvBuildStart").checked,
      notify_build_progress: $("#tgEvBuildProgress").checked,
      notify_build_done: $("#tgEvBuildDone").checked,
      notify_import: $("#tgEvImport").checked,
      notify_device_scan: $("#tgEvDevice").checked,
      notify_release: $("#tgEvRelease").checked,
    };

    const cfg = currentTgConfig || {};
    cfg.bot_token = token;
    cfg.enabled = enabled;
    cfg.poll_active = pollActive;
    cfg.events = events;

    try {
      await invoke("tg_save_config", { config: cfg });

      // Запуск или остановка фонового слушателя
      if (pollActive && token) {
        await invoke("tg_start_listener");
      } else {
        await invoke("tg_stop_listener");
      }

      toast("Настройки Telegram сохранены!", "pink");
      loadTelegramConfig();
    } catch (e) {
      toast("Ошибка сохранения: " + e);
    }
  };

  // Опрос команд бота прямо сейчас
  $("#btnTgPollNow").onclick = async () => {
    toast("Проверка входящих команд Telegram...", "pink");
    try {
      const count = await invoke("tg_poll_updates_once");
      if (count > 0) {
        toast(`Обработано ${count} новых команд!`, "pink");
      } else {
        toast("Новых команд не обнаружено");
      }
      loadTelegramConfig();
    } catch (e) {
      toast("Ошибка опроса: " + e);
    }
  };

  // Отправка тестового сообщения
  $("#btnTgSendTest").onclick = async () => {
    try {
      const res = await invoke("tg_send_test_message", { chatId: null });
      toast(res, "pink");
    } catch (e) {
      toast("Ошибка отправки: " + e);
    }
  };

  // Подписка на событие обновления каналов от бэкенда
  if (T && T.event) {
    T.event.listen("tg-channels-updated", ({ payload }) => {
      toast("Telegram: обнаружена новая привязка канала!", "pink");
      if (payload) renderTelegramChannels(payload);
    });
  }

  /* ---------- СБОРЩИК & REPACKER (data-page="unpack") ---------- */
  let currentBuildTargets = null;

  async function initRepackPage() {
    const sel = $("#repackProjectSelect");
    sel.innerHTML = '<option value="">Выберите проект...</option>';
    try {
      const items = await invoke("list_projects");
      if (items && items.length > 0) {
        items.forEach(it => {
          const opt = document.createElement("option");
          opt.value = it.meta.name;
          opt.textContent = `${it.meta.name} (${it.parts} образов)`;
          if (currentProject === it.meta.name) opt.selected = true;
          sel.append(opt);
        });
      }
      if (sel.value) {
        loadRepackTargets();
      } else if (items && items.length > 0) {
        sel.value = items[0].meta.name;
        loadRepackTargets();
      }
    } catch (e) {
      console.error(e);
    }
  }

  $("#repackProjectSelect").onchange = () => {
    loadRepackTargets();
  };
  $("#btnRefreshRepack").onclick = () => {
    loadRepackTargets();
  };
  $("#btnOpenOutDir").onclick = async () => {
    const proj = $("#repackProjectSelect").value;
    if (!proj) return toast("Сначала выберите проект");
    try {
      await invoke("open_in_explorer", { name: proj });
    } catch (e) {
      toast("Ошибка: " + e);
    }
  };

  async function loadRepackTargets() {
    const proj = $("#repackProjectSelect").value;
    const extList = $("#repackExtractedList");
    const superList = $("#superPartitionsCheckList");
    const outList = $("#repackOutList");

    if (!proj) {
      extList.innerHTML = '<div class="empty-state" style="padding:20px">Выберите проект выше</div>';
      superList.innerHTML = '<span class="muted" style="font-size:12px">Проект не выбран</span>';
      outList.innerHTML = '<div class="empty-state" style="padding:20px">Проект не выбран</div>';
      return;
    }

    try {
      const targets = await invoke("get_build_targets", { project: proj });
      currentBuildTargets = targets;

      // 1. Разобранные папки
      if (!targets.extracted || targets.extracted.length === 0) {
        extList.innerHTML = `<div class="empty-placeholder" style="padding:24px">
          <div class="ic">${icon('puzzle')}</div>
          <h3>Нет извлечённых разделов</h3>
          <p class="muted" style="font-size:12px">Перейдите в «Разделы и образы» и нажмите «Разобрать в папку».</p>
        </div>`;
      } else {
        extList.innerHTML = "";
        targets.extracted.forEach(part => {
          const row = document.createElement("div");
          row.className = "spec-card";
          row.style.background = "rgba(10,20,55,.6)";
          row.style.display = "flex";
          row.style.flexDirection = "row";
          row.style.alignItems = "center";
          row.style.justifyContent = "space-between";
          row.style.gap = "10px";

          const fsOptions = `
            <select class="search-input sel-part-fs" data-part="${part.name}" style="padding:4px 8px;font-size:12px;width:140px">
              <option value="erofs" ${part.detected_fs === 'erofs' ? 'selected' : ''}>EROFS (lz4hc)</option>
              <option value="boot" ${part.detected_fs === 'boot' ? 'selected' : ''}>Boot (MagiskBoot)</option>
              <option value="ext4" ${part.detected_fs === 'ext4' ? 'selected' : ''}>EXT4 (Linux)</option>
            </select>
          `;

          row.innerHTML = `
            <div style="display:flex;align-items:center;gap:10px;min-width:0;flex:1">
              <span style="font-size:20px">${icon(part.detected_fs === 'boot' ? 'zap' : 'folder', 20)}</span>
              <div style="min-width:0">
                <b style="color:var(--text);font-size:14px">${part.name}</b>
                <div style="font-size:11.5px;color:var(--muted)">
                  ${part.file_count} файлов • ${formatBytes(part.total_bytes)}
                  ${part.has_fs_config ? '<span style="color:#6ee7b7;margin-left:4px">✓ fs_config</span>' : ''}
                </div>
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:8px">
              ${fsOptions}
              <button class="btn sm primary btn-repack-single" data-part="${part.name}">${icon('hammer', 13)} Собрать</button>
            </div>
          `;
          extList.append(row);
        });

        $$(".btn-repack-single").forEach(b => {
          b.onclick = async () => {
            const pName = b.dataset.part;
            const selFs = $(`.sel-part-fs[data-part="${pName}"]`).value;
            toast(`Запуск сборки раздела ${pName} (${selFs})...`, "pink");
            try {
              await invoke("repack_partition", {
                project: proj,
                part: pName,
                fsType: selFs,
                compAlgorithm: "lz4hc,9"
              });
              loadRepackTargets();
            } catch (e) {
              toast("Ошибка сборки: " + e);
            }
          };
        });
      }

      // 2. Разделы для super.img
      if (!targets.images || targets.images.length === 0) {
        superList.innerHTML = '<span class="muted" style="font-size:12px">В images/ нет образов для super.img</span>';
        $("#superTotalCalcSize").textContent = "0 МБ";
      } else {
        superList.innerHTML = "";
        const superEligible = ["system", "vendor", "product", "system_ext", "odm", "system_dlkm", "vendor_dlkm"];
        targets.images.forEach(img => {
          const isRec = superEligible.some(e => img.name.toLowerCase().startsWith(e));
          const lbl = document.createElement("label");
          lbl.style.display = "flex";
          lbl.style.alignItems = "center";
          lbl.style.justifyContent = "space-between";
          lbl.style.fontSize = "12.5px";
          lbl.style.cursor = "pointer";

          lbl.innerHTML = `
            <div style="display:flex;align-items:center;gap:8px">
              <input type="checkbox" class="chk-super-part" value="${img.name}" data-bytes="${img.size}" ${isRec ? 'checked' : ''} style="accent-color:var(--pink);transform:scale(1.15)" />
              <span><b>${img.name}.img</b> ${img.is_sparse ? '<span class="badge sparse">sparse</span>' : ''}</span>
            </div>
            <span style="color:var(--muted)">${formatBytes(img.size)}</span>
          `;
          superList.append(lbl);
        });

        $$(".chk-super-part").forEach(chk => chk.onchange = updateSuperCalculatedSize);
        updateSuperCalculatedSize();
      }

      $("#superBadgeReady").className = "pill " + (targets.super_ready ? "ok" : "bad");
      $("#superBadgeReady").innerHTML = `<i></i>${targets.super_ready ? "Готов к сборке" : "Мало образов"}`;

      // 3. Файлы в out/
      $("#repackOutCount").textContent = targets.out_files.length;
      if (!targets.out_files || targets.out_files.length === 0) {
        outList.innerHTML = '<div class="empty-state" style="padding:20px">Папка out/ пуста. Запустите сборку выше.</div>';
      } else {
        outList.innerHTML = "";
        targets.out_files.forEach(f => {
          const item = document.createElement("div");
          item.className = "spec-card";
          item.style.background = "rgba(10,20,55,.6)";
          item.style.display = "flex";
          item.style.flexDirection = "row";
          item.style.alignItems = "center";
          item.style.justifyContent = "space-between";
          item.innerHTML = `
            <div style="display:flex;align-items:center;gap:8px">
              <span style="font-size:18px">${icon('package', 18)}</span>
              <b>${f}</b>
            </div>
            <span class="badge" style="background:rgba(142,197,255,.15);color:var(--sky)">ГОТОВ</span>
          `;
          outList.append(item);
        });
      }

    } catch (e) {
      console.error(e);
      toast("Ошибка загрузки целей: " + e);
    }
  }

  function updateSuperCalculatedSize() {
    let sum = 0;
    $$(".chk-super-part:checked").forEach(chk => {
      sum += parseInt(chk.dataset.bytes || "0", 10);
    });
    // Добавляем 8 МБ служебного запаса metadata
    const total = sum + 8 * 1024 * 1024;
    $("#superTotalCalcSize").textContent = formatBytes(total);
  }

  // Кнопка: Собрать все извлечённые разделы
  $("#btnRepackAllExtracted").onclick = async () => {
    const proj = $("#repackProjectSelect").value;
    if (!proj || !currentBuildTargets || currentBuildTargets.extracted.length === 0) {
      return toast("Нет разделов для сборки");
    }
    toast(`Запуск пакетной сборки всех разделов (${currentBuildTargets.extracted.length})...`, "pink");
    for (const part of currentBuildTargets.extracted) {
      const selFs = $(`.sel-part-fs[data-part="${part.name}"]`)?.value || part.detected_fs;
      try {
        await invoke("repack_partition", {
          project: proj,
          part: part.name,
          fsType: selFs,
          compAlgorithm: "lz4hc,9"
        });
      } catch (e) {
        toast(`Ошибка ${part.name}: ` + e);
      }
    }
    toast("Все извлечённые разделы пересобраны!", "pink");
    loadRepackTargets();
  };

  // Кнопка: Собрать super.img (lpmake)
  $("#btnRunRepackSuper").onclick = async () => {
    const proj = $("#repackProjectSelect").value;
    if (!proj) return toast("Сначала выберите проект");

    const parts = [];
    $$(".chk-super-part:checked").forEach(c => parts.push(c.value));
    if (parts.length === 0) {
      return toast("Отметьте хотя бы один раздел для super.img");
    }

    const sparse = $("#chkSuperSparse").checked;
    toast(`Сборка super.img из ${parts.length} разделов (lpmake)...`, "pink");
    try {
      await invoke("repack_super", {
        project: proj,
        partitions: parts,
        sparse: sparse,
        customSize: null
      });
      loadRepackTargets();
    } catch (e) {
      toast("Ошибка сборки super.img: " + e);
    }
  };

  // Кнопка: Сформировать Fastboot ROM
  $("#btnBuildFastbootPkg").onclick = async () => {
    const proj = $("#repackProjectSelect").value;
    if (!proj) return toast("Сначала выберите проект");

    const title = $("#fastbootRomTitle").value.trim() || proj;
    const author = $("#fastbootRomAuthor").value.trim() || "MeyaForge";

    toast(`Формирование Fastboot пакета: ${title}...`, "pink");
    try {
      await invoke("build_fastboot_package", {
        project: proj,
        romTitle: title,
        author: author
      });
      loadRepackTargets();
    } catch (e) {
      toast("Ошибка формирования пакета: " + e);
    }
  };

  /* ---------- ПОРТИРОВАНИЕ (data-page="port") ---------- */
  let currentComparison = null;

  async function initPortingPage() {
    const bSel = $("#portBaseSelect");
    const pSel = $("#portPortSelect");
    bSel.innerHTML = '<option value="">База (Сток)...</option>';
    pSel.innerHTML = '<option value="">Порт (Донор)...</option>';

    try {
      const items = await invoke("list_projects");
      if (items && items.length > 0) {
        items.forEach(it => {
          const opt1 = document.createElement("option");
          opt1.value = it.meta.name;
          opt1.textContent = it.meta.name;
          bSel.append(opt1);

          const opt2 = document.createElement("option");
          opt2.value = it.meta.name;
          opt2.textContent = it.meta.name;
          pSel.append(opt2);
        });

        if (items.length >= 2) {
          bSel.value = items[0].meta.name;
          pSel.value = items[1].meta.name;
        } else if (items.length === 1) {
          bSel.value = items[0].meta.name;
        }
      }
    } catch (e) {
      console.error(e);
    }
  }

  $("#btnCompareProjects").onclick = async () => {
    const base = $("#portBaseSelect").value;
    const port = $("#portPortSelect").value;

    if (!base || !port) return toast("Выберите оба проекта: Базу и Порт");
    if (base === port) return toast("База и Порт должны быть разными проектами");

    toast(`Глубокое сравнение: ${base} → ${port}...`, "pink");
    try {
      const res = await invoke("compare_projects", { baseName: base, portName: port });
      currentComparison = res;
      renderComparisonResult(res);
    } catch (e) {
      toast("Ошибка сравнения: " + e);
    }
  };

  $("#btnRecompareProjects").onclick = () => {
    $("#btnCompareProjects").click();
  };

  function renderComparisonResult(data) {
    $("#portEmptyState").classList.add("hidden");
    $("#portResultView").classList.remove("hidden");

    // 1. Оценка совместимости
    const scoreVal = $("#portScoreVal");
    scoreVal.textContent = data.compatibility_score + "%";
    scoreVal.style.color = data.compatibility_score >= 80 ? "#6ee7b7" : data.compatibility_score >= 50 ? "#fbbf24" : "#ff6b8a";
    $("#portScoreIcon").innerHTML = icon(data.compatibility_score >= 80 ? 'sparkles' : data.compatibility_score >= 50 ? 'alert' : 'ban', 36);

    const notesWrap = $("#portScoreNotes");
    notesWrap.innerHTML = "";
    data.compatibility_notes.forEach(n => {
      const d = document.createElement("div");
      d.textContent = n;
      notesWrap.append(d);
    });

    // 2. Спецификации Базы vs Порта
    $("#portBaseTitle").textContent = `База: ${data.base_name}`;
    $("#portPortTitle").textContent = `Порт: ${data.port_name}`;

    function renderPropsList(el, p) {
      el.innerHTML = `
        <div class="kv-row"><span>Android OS</span><b>${p.android_version}</b></div>
        <div class="kv-row"><span>SDK API</span><b>${p.sdk_level}</b></div>
        <div class="kv-row"><span>Патч безопасности</span><b>${p.security_patch}</b></div>
        <div class="kv-row"><span>Устройство (device)</span><b>${p.device}</b></div>
        <div class="kv-row"><span>Модель (model)</span><b>${p.model}</b></div>
        <div class="kv-row"><span>Бренд</span><b>${p.brand}</b></div>
        <div class="kv-row"><span>Архитектура CPU</span><b>${p.cpu_abi}</b></div>
        <div class="kv-row"><span>VNDK версия</span><b>${p.vndk_version}</b></div>
        <div class="kv-row" style="flex-direction:column;gap:4px">
          <span>Fingerprint</span>
          <b class="code-sm" style="max-width:100%">${p.fingerprint}</b>
        </div>
      `;
    }

    renderPropsList($("#portBasePropsList"), data.base_props);
    renderPropsList($("#portPortPropsList"), data.port_props);

    // 3. Разделы
    $("#portCommonPartsCount").textContent = data.common_partitions.length;
    const bCommon = $("#portCommonPartsBadges");
    bCommon.innerHTML = data.common_partitions.map(p => `<span class="badge erofs">${p}</span>`).join(" ");

    const bBase = $("#portBaseOnlyBadges");
    bBase.innerHTML = data.base_only_partitions.length ? data.base_only_partitions.map(p => `<span class="badge raw">${p}</span>`).join(" ") : '<span class="muted">нет</span>';

    const bPort = $("#portPortOnlyBadges");
    bPort.innerHTML = data.port_only_partitions.length ? data.port_only_partitions.map(p => `<span class="badge raw">${p}</span>`).join(" ") : '<span class="muted">нет</span>';

    // 4. HAL Модули
    const tbody = $("#portHalTbody");
    tbody.innerHTML = "";
    if (!data.hal_modules || data.hal_modules.length === 0) {
      tbody.innerHTML = '<tr><td colspan="4" style="text-align:center;padding:14px;color:var(--muted)">HAL модули не обнаружены (распакуйте раздел vendor)</td></tr>';
    } else {
      data.hal_modules.forEach(m => {
        const tr = document.createElement("tr");
        tr.innerHTML = `
          <td><span class="badge raw">${m.category}</span></td>
          <td><code style="color:var(--sky)">${m.name}</code></td>
          <td>${m.in_base ? '<span style="color:#6ee7b7">✓ Да</span>' : '<span style="color:var(--muted)">—</span>'}</td>
          <td>${m.in_port ? '<span style="color:#6ee7b7">✓ Да</span>' : '<span style="color:var(--muted)">—</span>'}</td>
        `;
        tbody.append(tr);
      });
    }
  }

  // Кнопка: Применить портирование
  $("#btnApplyPortingTransfer").onclick = async () => {
    if (!currentComparison) return toast("Сначала выполните сравнение прошивок");
    const base = currentComparison.base_name;
    const port = currentComparison.port_name;

    const opts = {
      transfer_kernel: $("#chkPortKernel").checked,
      transfer_fstab: $("#chkPortFstab").checked,
      sync_hal_blobs: $("#chkPortHal").checked,
      sync_firmware: $("#chkPortFirmware").checked,
      patch_build_props: $("#chkPortProps").checked,
      remove_avb_dm_verity: $("#chkPortAvb").checked,
    };

    toast(`Применение операций портирования: ${base} → ${port}...`, "pink");
    try {
      await invoke("apply_porting_transfer", {
        baseName: base,
        portName: port,
        options: opts,
      });
      toast("Портирование успешно завершено! Теперь вы можете пересобрать прошивку.", "pink");
    } catch (e) {
      toast("Ошибка портирования: " + e);
    }
  };

  /* ---------- ДЕБЛОАТЕР ПРОШИВОК (data-page="debloat") ---------- */
  let currentDebloatData = null;
  let selectedDebloatPaths = new Set();
  let currentDebloatCategory = "all";

  async function initDebloatPage() {
    const sel = $("#debloatProjectSelect");
    sel.innerHTML = '<option value="">Выберите проект...</option>';
    try {
      const items = await invoke("list_projects");
      if (items && items.length > 0) {
        items.forEach(it => {
          const opt = document.createElement("option");
          opt.value = it.meta.name;
          opt.textContent = `${it.meta.name} (${it.parts} образов)`;
          if (currentProject === it.meta.name) opt.selected = true;
          sel.append(opt);
        });
        if (!sel.value && items.length > 0) {
          sel.value = items[0].meta.name;
        }
      }
      if (sel.value) {
        runDebloatScan(sel.value);
      }
    } catch (e) {
      console.error(e);
    }
  }

  $("#debloatProjectSelect").onchange = (e) => {
    if (e.target.value) {
      runDebloatScan(e.target.value);
    }
  };

  $("#btnScanDebloat").onclick = () => {
    const proj = $("#debloatProjectSelect").value;
    if (!proj) return toast("Сначала выберите проект!", "pink");
    runDebloatScan(proj);
  };

  function updateDebloatSelectionSummary() {
    $("#debSelectedCount").textContent = selectedDebloatPaths.size;
    let selectedBytes = 0;
    if (currentDebloatData) {
      currentDebloatData.items.forEach(it => {
        if (selectedDebloatPaths.has(it.full_path)) {
          selectedBytes += it.size_bytes;
        }
      });
    }
    $("#debTotalFreed").textContent = formatBytes(selectedBytes);
  }

  function renderDebloatTable(filterText = "") {
    const tbody = $("#debloatTbody");
    tbody.innerHTML = "";

    if (!currentDebloatData || currentDebloatData.items.length === 0) {
      tbody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:30px;color:var(--muted)">В проекте не найдено приложений из базы мусора и рекламы. Прошивка чистая!</td></tr>`;
      return;
    }

    const q = filterText.toLowerCase().trim();
    const list = currentDebloatData.items.filter(it => {
      const matchCat = currentDebloatCategory === "all" || it.category === currentDebloatCategory;
      if (!matchCat) return false;
      if (!q) return true;
      return it.app_name.toLowerCase().includes(q)
        || it.package_name.toLowerCase().includes(q)
        || it.rel_path.toLowerCase().includes(q)
        || it.description.toLowerCase().includes(q);
    });

    if (list.length === 0) {
      tbody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:24px;color:var(--muted)">Нет приложений по выбранному фильтру.</td></tr>`;
      return;
    }

    list.forEach(item => {
      const tr = document.createElement("tr");
      const isChecked = selectedDebloatPaths.has(item.full_path);
      const badgeCat = item.category === "ad_telemetry" ? "raw" : (item.category === "google_heavy" ? "ext4" : "erofs");

      tr.innerHTML = `
        <td><input type="checkbox" class="chk-deb-item" data-path="${escapeHtml(item.full_path)}" ${isChecked ? 'checked' : ''} /></td>
        <td>
          <b>${escapeHtml(item.app_name)}</b><br>
          <small class="code-sm" style="color:var(--muted)">${escapeHtml(item.package_name)}</small>
        </td>
        <td>
          <span style="font-size:12.5px">${escapeHtml(item.description)}</span><br>
          <span class="badge ${badgeCat}" style="font-size:10px;margin-top:2px">${item.category}</span>
        </td>
        <td><code class="code-sm">${escapeHtml(item.rel_path)}</code></td>
        <td><b style="color:var(--sky)">${formatBytes(item.size_bytes)}</b></td>
      `;

      const chk = tr.querySelector(".chk-deb-item");
      chk.onchange = (e) => {
        if (e.target.checked) {
          selectedDebloatPaths.add(item.full_path);
        } else {
          selectedDebloatPaths.delete(item.full_path);
        }
        updateDebloatSelectionSummary();
      };

      tbody.appendChild(tr);
    });
  }

  async function runDebloatScan(project) {
    try {
      toast(`Сканирование приложений в ${project}...`, "pink");
      const res = await invoke("debloat_scan", { project });
      currentDebloatData = res;
      selectedDebloatPaths.clear();

      // По умолчанию выбираем все безопасные к удалению
      res.items.forEach(it => {
        if (it.safe_to_remove) {
          selectedDebloatPaths.add(it.full_path);
        }
      });

      $("#debloatSummaryBox").classList.remove("hidden");
      $("#debTotalFound").textContent = res.total_apps_found;
      $("#debBloatCount").textContent = res.bloat_apps_count;
      updateDebloatSelectionSummary();

      renderDebloatTable();
      toast(`Найдено ${res.bloat_apps_count} приложений для очистки`, "sky");
    } catch (e) {
      toast("Ошибка сканирования: " + e);
    }
  }

  $$("#debloatFilters .f-pill[data-debcat]").forEach(btn => {
    btn.onclick = () => {
      $$("#debloatFilters .f-pill[data-debcat]").forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      currentDebloatCategory = btn.dataset.debcat;
      renderDebloatTable($("#inputDebloatFilter").value);
    };
  });

  $("#btnDebloatSelectAll").onclick = () => {
    if (!currentDebloatData) return;
    currentDebloatData.items.forEach(it => selectedDebloatPaths.add(it.full_path));
    renderDebloatTable($("#inputDebloatFilter").value);
    updateDebloatSelectionSummary();
  };

  $("#btnDebloatDeselectAll").onclick = () => {
    selectedDebloatPaths.clear();
    renderDebloatTable($("#inputDebloatFilter").value);
    updateDebloatSelectionSummary();
  };

  $("#chkDebloatHeader").onchange = (e) => {
    if (e.target.checked) {
      $("#btnDebloatSelectAll").click();
    } else {
      $("#btnDebloatDeselectAll").click();
    }
  };

  $("#inputDebloatFilter").oninput = (e) => {
    renderDebloatTable(e.target.value);
  };

  $("#btnExecuteDebloat").onclick = async () => {
    const proj = $("#debloatProjectSelect").value;
    if (!proj) return toast("Сначала выберите проект!", "pink");
    if (selectedDebloatPaths.size === 0) return toast("Не выбрано ни одного приложения для удаления");

    const backupMode = $("#chkDebloatBackup").checked;
    const msg = backupMode
      ? `Переместить ${selectedDebloatPaths.size} приложений в removed_bloat/ (с возможностью восстановления)?`
      : `ВНИМАНИЕ: Безвозвратно удалить ${selectedDebloatPaths.size} приложений из проекта?`;

    if (!confirm(msg)) return;

    try {
      const res = await invoke("debloat_remove", {
        req: {
          project: proj,
          target_paths: Array.from(selectedDebloatPaths),
          backup_mode: backupMode
        }
      });

      toast(`Успешно удалено/перемещено ${res.removed_count} приложений! Освобождено ${formatBytes(res.freed_bytes)}`, "sky");
      runDebloatScan(proj);
    } catch (e) {
      toast("Ошибка деблоатинга: " + e);
    }
  };

  $("#btnRestoreDebloat").onclick = async () => {
    const proj = $("#debloatProjectSelect").value;
    if (!proj) return toast("Сначала выберите проект!", "pink");
    if (!confirm(`Восстановить все удаленные приложения из removed_bloat/ обратно в проект ${proj}?`)) return;

    try {
      const msg = await invoke("debloat_restore_all", { project: proj });
      toast(msg, "sky");
      runDebloatScan(proj);
    } catch (e) {
      toast("Ошибка восстановления: " + e);
    }
  };

  /* ---------- ФАЙЛОВЫЙ МЕНЕДЖЕР & ВСТРОЕННЫЙ РЕДАКТОР (data-page="files") ---------- */
  let fmCurrentProject = null;
  let fmCurrentFile = null;
  let fmExpandedFolders = new Set();
  let fmNewItemType = "file";
  let fmRenameTarget = null;

  async function initFileManager() {
    const sel = $("#fmProjectSelect");
    sel.innerHTML = '<option value="">Выберите проект...</option>';
    try {
      const items = await invoke("list_projects");
      if (items && items.length > 0) {
        items.forEach(it => {
          const opt = document.createElement("option");
          opt.value = it.meta.name;
          opt.textContent = `${it.meta.name} (${it.parts} образов)`;
          if (currentProject === it.meta.name || fmCurrentProject === it.meta.name) {
            opt.selected = true;
          }
          sel.append(opt);
        });
        if (!sel.value && items.length > 0) {
          sel.value = items[0].meta.name;
        }
      }
      fmCurrentProject = sel.value;
      if (fmCurrentProject) {
        loadFmTree();
      } else {
        $("#fmTreeBody").innerHTML = '<div class="empty-state" style="padding:20px;font-size:12px">Нет проектов. Создайте проект на вкладке «Проекты».</div>';
      }
    } catch (e) {
      console.error(e);
    }
  }

  $("#fmProjectSelect").onchange = () => {
    fmCurrentProject = $("#fmProjectSelect").value;
    fmCurrentFile = null;
    fmExpandedFolders.clear();
    resetFmEditor();
    loadFmTree();
  };

  $("#btnFmRefresh").onclick = () => {
    loadFmTree();
  };

  $("#btnFmOpenExplorer").onclick = async () => {
    if (!fmCurrentProject) return toast("Сначала выберите проект");
    try {
      await invoke("open_in_explorer", { name: fmCurrentProject });
    } catch (e) {
      toast("Ошибка: " + e);
    }
  };

  // Загрузка дерева проекта
  async function loadFmTree() {
    if (!fmCurrentProject) return;
    const body = $("#fmTreeBody");
    body.innerHTML = '<div style="padding:14px;color:var(--muted);font-size:12px">Загрузка структуры...</div>';

    try {
      const rootEntries = await invoke("fs_list_dir", { project: fmCurrentProject, relPath: null });
      body.innerHTML = "";
      if (!rootEntries || rootEntries.length === 0) {
        body.innerHTML = '<div class="empty-state" style="padding:20px;font-size:12px">Папка проекта пуста</div>';
        return;
      }
      renderFmNodes(body, rootEntries, 0);
    } catch (e) {
      body.innerHTML = `<div class="form-err" style="padding:10px">Ошибка: ${e}</div>`;
    }
  }

  function getFileIcon(name, ext, isDir) {
    if (isDir) return icon("folder", 14);
    const lName = name.toLowerCase();
    const lExt = ext.toLowerCase();

    if (lName === "build.prop" || lExt === "prop" || lExt === "cfg") return icon("settings", 14);
    if (lExt === "rc" || lExt === "sh" || lExt === "bat") return icon("scroll", 14);
    if (lExt === "xml" || lExt === "json") return icon("fileText", 14);
    if (lExt === "img" || lExt === "bin") return icon("disc", 14);
    if (lExt === "so") return icon("puzzle", 14);
    if (lExt === "apk" || lExt === "jar") return icon("smartphone", 14);
    if (["png", "jpg", "jpeg", "webp", "bmp", "svg"].includes(lExt)) return icon("image", 14);
    if (lName.includes("fstab") || lExt === "fstab") return icon("archive", 14);
    if (lExt === "txt" || lExt === "list" || lExt === "cpio") return icon("file", 14);
    return icon("file", 14);
  }

  function renderFmNodes(container, entries, level) {
    entries.forEach(entry => {
      const node = document.createElement("div");
      node.className = "fm-node";

      const row = document.createElement("div");
      row.className = "fm-row" + (fmCurrentFile && fmCurrentFile.rel_path === entry.rel_path ? " active" : "");
      row.style.paddingLeft = (level * 14 + 6) + "px";
      row.dataset.path = entry.rel_path;

      const chevron = entry.is_dir
        ? `<span class="fm-chevron ${fmExpandedFolders.has(entry.rel_path) ? 'expanded' : ''}">${icon('chevronRight')}</span>`
        : `<span style="width:14px"></span>`;

      const fileIcon = getFileIcon(entry.name, entry.ext, entry.is_dir);

      row.innerHTML = `
        ${chevron}
        <span style="font-size:14px">${fileIcon}</span>
        <span class="fm-name" title="${entry.name}">${entry.name}</span>
        ${entry.is_dir ? '' : `<span class="fm-size">${formatBytes(entry.size)}</span>`}
        <div class="fm-actions">
          <span class="icon-btn btn-fm-ren" title="Переименовать" style="font-size:11px">${icon('pencil')}</span>
          <span class="icon-btn btn-fm-del" title="Удалить" style="font-size:11px">${icon('trash')}</span>
        </div>
      `;

      let childrenContainer = null;
      if (entry.is_dir) {
        childrenContainer = document.createElement("div");
        childrenContainer.className = "fm-children" + (fmExpandedFolders.has(entry.rel_path) ? " open" : "");
      }

      // Клик по строке
      row.onclick = async (e) => {
        if (e.target.closest(".btn-fm-ren")) {
          e.stopPropagation();
          openFmRename(entry.rel_path);
          return;
        }
        if (e.target.closest(".btn-fm-del")) {
          e.stopPropagation();
          deleteFmItem(entry.rel_path, entry.name, entry.is_dir);
          return;
        }

        if (entry.is_dir) {
          const ch = row.querySelector(".fm-chevron");
          if (fmExpandedFolders.has(entry.rel_path)) {
            fmExpandedFolders.delete(entry.rel_path);
            ch.classList.remove("expanded");
            childrenContainer.classList.remove("open");
          } else {
            fmExpandedFolders.add(entry.rel_path);
            ch.classList.add("expanded");
            childrenContainer.classList.add("open");

            if (childrenContainer.childElementCount === 0) {
              childrenContainer.innerHTML = '<div style="font-size:11px;color:var(--muted);padding:4px">Загрузка...</div>';
              try {
                const subEntries = await invoke("fs_list_dir", { project: fmCurrentProject, relPath: entry.rel_path });
                childrenContainer.innerHTML = "";
                if (!subEntries || subEntries.length === 0) {
                  childrenContainer.innerHTML = '<div style="font-size:11px;color:var(--muted);padding:4px">Папка пуста</div>';
                } else {
                  renderFmNodes(childrenContainer, subEntries, level + 1);
                }
              } catch (err) {
                childrenContainer.innerHTML = `<div class="form-err" style="font-size:11px">${err}</div>`;
              }
            }
          }
        } else {
          // Клик по файлу: открываем в редакторе
          $$(".fm-row").forEach(r => r.classList.remove("active"));
          row.classList.add("active");
          openFmFile(entry);
        }
      };

      node.append(row);
      if (childrenContainer) node.append(childrenContainer);
      container.append(node);
    });
  }

  // Фильтр файлов в дереве
  $("#fmTreeFilter").oninput = (e) => {
    const q = e.target.value.trim().toLowerCase();
    $$(".fm-row").forEach(r => {
      const name = r.querySelector(".fm-name")?.textContent.toLowerCase() || "";
      if (!q || name.includes(q)) {
        r.style.display = "flex";
      } else {
        r.style.display = "none";
      }
    });
  };

  // Открытие файла в редакторе
  async function openFmFile(entry) {
    fmCurrentFile = entry;
    $("#fmCrumbs").innerHTML = `<span>${fmCurrentProject}</span> / <b>${entry.rel_path}</b>`;
    $("#btnFmCopyPath").style.display = "inline-flex";
    $("#btnFmDeleteCurrent").style.display = "inline-flex";

    // Сброс всех экранов
    $("#fmEmptyEditor").classList.add("hidden");
    $("#fmTextEditorWrap").classList.add("hidden");
    $("#fmImageViewWrap").classList.add("hidden");
    $("#fmBinaryViewWrap").classList.add("hidden");
    $("#btnFmSaveFile").style.display = "none";
    $("#fmLangBadge").style.display = "none";
    $("#fmStatsBadge").textContent = "";

    try {
      const fileData = await invoke("fs_read_file", { project: fmCurrentProject, relPath: entry.rel_path });

      // Определение режима отображения
      const lExt = entry.ext.toLowerCase();
      const isImg = ["png", "jpg", "jpeg", "webp", "bmp", "svg"].includes(lExt);

      if (isImg) {
        $("#fmImageViewWrap").classList.remove("hidden");
        // Получаем прямой путь через asset протокол или base64
        $("#fmImagePreview").src = `https://asset.localhost/${entry.rel_path}`;
        $("#fmImageInfo").textContent = `${entry.name} • ${formatBytes(entry.size)}`;
        return;
      }

      if (fileData.is_binary) {
        $("#fmBinaryViewWrap").classList.remove("hidden");
        $("#fmBinaryName").textContent = entry.name;
        $("#fmBinaryDesc").textContent = `Бинарный файл (${formatBytes(entry.size)}). Прямое текстовое редактирование отключено для предотвращения повреждения структуры.`;
        return;
      }

      // Текстовый редактор
      $("#fmTextEditorWrap").classList.remove("hidden");
      $("#btnFmSaveFile").style.display = "inline-flex";
      $("#fmLangBadge").style.display = "inline-block";

      const txt = fileData.content || "";
      const textarea = $("#fmCodeTextarea");
      textarea.value = txt;

      // Определение языка
      let lang = "TXT";
      const lName = entry.name.toLowerCase();
      if (lName === "build.prop" || lExt === "prop" || lExt === "cfg") lang = "PROP";
      else if (lExt === "rc") lang = "RC";
      else if (lExt === "xml") lang = "XML";
      else if (lExt === "json") lang = "JSON";
      else if (lExt === "sh" || lExt === "bat") lang = "SHELL";
      else if (lName.includes("fstab") || lExt === "fstab") lang = "FSTAB";

      $("#fmLangBadge").textContent = lang;

      updateLineNumbers(txt);
      updateFmStats(txt, entry.size);

    } catch (e) {
      toast("Ошибка чтения: " + e);
    }
  }

  function updateLineNumbers(text) {
    const lines = text.split("\n").length;
    const gutter = $("#fmLineNumbers");
    let nums = "";
    for (let i = 1; i <= lines; i++) {
      nums += i + "\n";
    }
    gutter.textContent = nums;
  }

  function updateFmStats(text, sizeBytes) {
    const lines = text.split("\n").length;
    $("#fmStatsBadge").textContent = `${lines} строк • ${formatBytes(sizeBytes || text.length)}`;
  }

  // Синхронизация скролла редактора и номеров строк
  const fmTa = $("#fmCodeTextarea");
  fmTa.onscroll = () => {
    $("#fmLineNumbers").scrollTop = fmTa.scrollTop;
  };

  fmTa.oninput = () => {
    updateLineNumbers(fmTa.value);
    updateFmStats(fmTa.value, fmTa.value.length);
  };

  // Клавиатурные шорткаты в редакторе (Ctrl+S, Tab)
  fmTa.onkeydown = (e) => {
    if ((e.ctrlKey || e.metaKey) && (e.key === "s" || e.key === "S")) {
      e.preventDefault();
      saveCurrentFmFile();
      return;
    }

    if (e.key === "Tab") {
      e.preventDefault();
      const start = fmTa.selectionStart;
      const end = fmTa.selectionEnd;
      fmTa.value = fmTa.value.substring(0, start) + "  " + fmTa.value.substring(end);
      fmTa.selectionStart = fmTa.selectionEnd = start + 2;
      updateLineNumbers(fmTa.value);
    }
  };

  // Кнопка: Сохранить файл
  $("#btnFmSaveFile").onclick = () => {
    saveCurrentFmFile();
  };

  async function saveCurrentFmFile() {
    if (!fmCurrentProject || !fmCurrentFile) return;
    const content = $("#fmCodeTextarea").value;
    try {
      await invoke("fs_write_file", {
        project: fmCurrentProject,
        relPath: fmCurrentFile.rel_path,
        content: content
      });
      toast(`✓ ${fmCurrentFile.name} успешно сохранён`, "pink");
    } catch (e) {
      toast("Ошибка сохранения: " + e);
    }
  }

  // Кнопка: Копировать путь
  $("#btnFmCopyPath").onclick = () => {
    if (!fmCurrentFile) return;
    navigator.clipboard.writeText(fmCurrentFile.rel_path);
    toast("Путь скопирован в буфер обмена!");
  };

  // Кнопка: Удалить текущий открытый файл
  $("#btnFmDeleteCurrent").onclick = () => {
    if (!fmCurrentFile) return;
    deleteFmItem(fmCurrentFile.rel_path, fmCurrentFile.name, false);
  };

  // Открытие бинарного файла через проводник
  $("#btnFmOpenBinaryExt").onclick = async () => {
    if (!fmCurrentProject) return;
    try {
      await invoke("open_in_explorer", { name: fmCurrentProject });
    } catch (e) {
      toast("Ошибка: " + e);
    }
  };

  function resetFmEditor() {
    $("#fmCrumbs").innerHTML = `<span>Файл не выбран</span>`;
    $("#btnFmCopyPath").style.display = "none";
    $("#btnFmDeleteCurrent").style.display = "none";
    $("#btnFmSaveFile").style.display = "none";
    $("#fmLangBadge").style.display = "none";
    $("#fmStatsBadge").textContent = "";

    $("#fmEmptyEditor").classList.remove("hidden");
    $("#fmTextEditorWrap").classList.add("hidden");
    $("#fmImageViewWrap").classList.add("hidden");
    $("#fmBinaryViewWrap").classList.add("hidden");
  }

  // Удаление файла / папки
  function deleteFmItem(relPath, name, isDir) {
    showConfirmDialog({
      title: `Удаление ${isDir ? 'папки' : 'файла'}`,
      desc: `Удалить ${isDir ? 'папку' : 'файл'} <b>«${name}»</b> (${relPath})? Это действие необратимо.`,
      iconName: "trash",
      okText: "Удалить",
      okDanger: true,
      onConfirm: async () => {
        try {
          await invoke("fs_delete_entry", { project: fmCurrentProject, relPath: relPath });
          toast(`Удалено: ${name}`);
          if (fmCurrentFile && fmCurrentFile.rel_path === relPath) {
            resetFmEditor();
            fmCurrentFile = null;
          }
          loadFmTree();
        } catch (e) {
          toast("Ошибка удаления: " + e);
        }
      }
    });
  }

  // Модалка: Создать файл / папку
  $("#btnFmNewFile").onclick = () => {
    if (!fmCurrentProject) return toast("Сначала выберите проект");
    fmNewItemType = "file";
    $("#fmNewItemTitle").textContent = "Создать файл";
    $("#fmNewItemDesc").textContent = `Создание нового файла в проекте ${fmCurrentProject}:`;
    $("#fmNewItemPlaceholder").textContent = "Путь/Имя (например, config/custom.prop)";
    $("#fmNewItemInput").value = "";
    $("#fmNewItemModal").classList.remove("hidden");
    $("#fmNewItemInput").focus();
  };

  $("#btnFmNewFolder").onclick = () => {
    if (!fmCurrentProject) return toast("Сначала выберите проект");
    fmNewItemType = "folder";
    $("#fmNewItemTitle").textContent = "Создать папку";
    $("#fmNewItemDesc").textContent = `Создание новой папки в проекте ${fmCurrentProject}:`;
    $("#fmNewItemPlaceholder").textContent = "Путь/Имя (например, extracted/system/my_folder)";
    $("#fmNewItemInput").value = "";
    $("#fmNewItemModal").classList.remove("hidden");
    $("#fmNewItemInput").focus();
  };

  $("#btnCancelFmNewItem").onclick = () => {
    $("#fmNewItemModal").classList.add("hidden");
  };

  $("#btnConfirmFmNewItem").onclick = async () => {
    const val = $("#fmNewItemInput").value.trim();
    if (!val) return toast("Введите имя");
    try {
      if (fmNewItemType === "file") {
        await invoke("fs_create_file", { project: fmCurrentProject, relPath: val });
        toast(`Файл создан: ${val}`, "pink");
      } else {
        await invoke("fs_create_folder", { project: fmCurrentProject, relPath: val });
        toast(`Папка создана: ${val}`, "pink");
      }
      $("#fmNewItemModal").classList.add("hidden");
      loadFmTree();
    } catch (e) {
      toast("Ошибка: " + e);
    }
  };

  // Модалка: Переименовать
  function openFmRename(relPath) {
    fmRenameTarget = relPath;
    $("#fmRenameInput").value = relPath;
    $("#fmRenameModal").classList.remove("hidden");
    $("#fmRenameInput").focus();
  }

  $("#btnCancelFmRename").onclick = () => {
    $("#fmRenameModal").classList.add("hidden");
  };

  $("#btnConfirmFmRename").onclick = async () => {
    const newName = $("#fmRenameInput").value.trim();
    if (!newName || !fmRenameTarget) return toast("Введите новое имя");
    try {
      await invoke("fs_rename_entry", {
        project: fmCurrentProject,
        oldRelPath: fmRenameTarget,
        newRelPath: newName
      });
      toast("Объект переименован!");
      $("#fmRenameModal").classList.add("hidden");
      loadFmTree();
    } catch (e) {
      toast("Ошибка: " + e);
    }
  };

  /* ---------- APK KITCHEN ---------- */
  let apkKitchenProject = null;

  async function initApkKitchen() {
    const sel = $("#apkProjectSelect");
    sel.innerHTML = '<option value="">Выберите проект...</option>';
    try {
      const items = await invoke("list_projects");
      (items || []).forEach(it => {
        const opt = document.createElement("option");
        opt.value = it.meta.name;
        opt.textContent = `${it.meta.name} (${it.parts} образов)`;
        if (currentProject === it.meta.name || apkKitchenProject === it.meta.name) opt.selected = true;
        sel.append(opt);
      });
      if (!sel.value && items && items.length > 0) sel.value = items[0].meta.name;
      apkKitchenProject = sel.value;
      await apkRefreshAll();
    } catch (e) {
      console.error(e);
    }
  }

  $("#apkProjectSelect").onchange = () => {
    apkKitchenProject = $("#apkProjectSelect").value;
    apkRefreshAll();
  };
  $("#btnApkRefresh").onclick = () => apkRefreshAll();

  async function apkRefreshAll() {
    const scanBody = $("#apkScanTbody");
    const decBody = $("#apkDecTbody");
    if (!apkKitchenProject) {
      scanBody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:20px;color:var(--muted)">Выберите проект для сканирования приложений.</td></tr>`;
      decBody.innerHTML = "";
      $("#apkFoundCount").textContent = "0";
      $("#apkOdexCount").textContent = "0";
      $("#apkDecCount").textContent = "0";
      return;
    }

    // APK в прошивке
    try {
      const apks = (await invoke("apk_scan_project", { project: apkKitchenProject })) || [];
      $("#apkFoundCount").textContent = apks.length;
      $("#apkOdexCount").textContent = apks.filter(a => a.has_odex).length;
      scanBody.innerHTML = "";
      if (apks.length === 0) {
        scanBody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:20px;color:var(--muted)">APK не найдены. Сначала разберите разделы прошивки (system, product, vendor...).</td></tr>`;
      }
      apks.forEach(a => {
        const tr = document.createElement("tr");
        tr.innerHTML = `
          <td><b>${escapeHtml(a.name)}</b></td>
          <td style="color:var(--muted);font-family:Consolas,monospace;font-size:11px;word-break:break-all">${escapeHtml(a.rel_path)}</td>
          <td>${formatBytes(a.size)}</td>
          <td>${a.has_odex ? `<span class="badge raw">${a.odex_count} odex</span>` : '<span class="badge erofs">deodexed</span>'}</td>
          <td style="text-align:right;white-space:nowrap">
            <button class="btn sm primary btn-apk-dec">${icon('package', 13)} Декомпилировать</button>
            ${a.has_odex ? `<button class="btn sm ghost btn-apk-deodex">${icon('zap', 13)} Деодекс</button>` : ''}
          </td>
        `;
        tr.querySelector(".btn-apk-dec").onclick = async () => {
          try {
            await invoke("apk_decompile_project", { project: apkKitchenProject, relPath: a.rel_path });
            toast(`Запущена декомпиляция ${a.name}...`, "pink");
          } catch (e) { toast("Ошибка: " + e); }
        };
        const bDeodex = tr.querySelector(".btn-apk-deodex");
        if (bDeodex) {
          bDeodex.onclick = () => {
            askDeviceConfirmation(
              `Деодекс ${a.name}`,
              `Будет выполнена конвертация <b>odex → classes.dex</b> через baksmali/smali с bootclasspath из framework проекта.<br><br>
              Результат: <code>out/${a.name.replace(/\.apk$/i, "")}-deodexed.apk</code>. После деодекса приложение можно декомпилировать Apktool с полным кодом.`,
              async () => {
                try {
                  await invoke("apk_deodex", { project: apkKitchenProject, relPath: a.rel_path });
                  toast(`Запущен деодекс ${a.name}...`, "pink");
                } catch (e) { toast("Ошибка: " + e); }
              }
            );
          };
        }
        scanBody.appendChild(tr);
      });
    } catch (e) {
      scanBody.innerHTML = `<tr><td colspan="5" style="text-align:center;padding:16px;color:#ff8fab">Ошибка сканирования: ${e}</td></tr>`;
    }

    // Декомпилированные
    try {
      const list = (await invoke("apk_list_decompiled", { project: apkKitchenProject })) || [];
      $("#apkDecCount").textContent = list.length;
      decBody.innerHTML = "";
      if (list.length === 0) {
        decBody.innerHTML = `<tr><td colspan="4" style="text-align:center;padding:20px;color:var(--muted)">Нет декомпилированных приложений. Декомпилируйте APK из таблицы выше или с диска.</td></tr>`;
      }
      list.forEach(d => {
        const tr = document.createElement("tr");
        const contents = [
          d.has_smali ? '<span class="badge ext4">smali</span>' : '',
          d.has_resources ? '<span class="badge erofs">res</span>' : ''
        ].filter(Boolean).join(" ") || '<span class="muted">—</span>';
        tr.innerHTML = `
          <td><b>${escapeHtml(d.name)}</b><br><small style="color:var(--muted);font-family:Consolas,monospace;font-size:11px">${escapeHtml(d.folder)}</small></td>
          <td>${contents}</td>
          <td>${d.file_count}</td>
          <td style="text-align:right;white-space:nowrap">
            <button class="btn sm ghost btn-apk-edit" title="Открыть в файловом менеджере">${icon('pencil', 13)} Редактор</button>
            <button class="btn sm ghost btn-apk-build">${icon('hammer', 13)} Собрать</button>
            <button class="btn sm primary btn-apk-sign">${icon('zap', 13)} Собрать & Подписать</button>
            <button class="btn sm danger btn-apk-del" title="Удалить папку">${icon('trash', 13)}</button>
          </td>
        `;
        tr.querySelector(".btn-apk-edit").onclick = () => fmRevealPath(apkKitchenProject, d.folder);
        tr.querySelector(".btn-apk-build").onclick = async () => {
          try {
            await invoke("build_apk", { project: apkKitchenProject, targetFolder: d.name, sign: false });
            toast(`Запущена сборка ${d.name} (без подписи)...`, "pink");
          } catch (e) { toast("Ошибка: " + e); }
        };
        tr.querySelector(".btn-apk-sign").onclick = async () => {
          try {
            await invoke("build_apk", { project: apkKitchenProject, targetFolder: d.name, sign: true });
            toast(`Запущена сборка и подпись ${d.name}...`, "pink");
          } catch (e) { toast("Ошибка: " + e); }
        };
        tr.querySelector(".btn-apk-del").onclick = () => {
          showConfirmDialog({
            title: `Удаление ${d.name}`,
            desc: `Удалить папку <b>${d.folder}</b> со всем декомпилированным содержимым?`,
            iconName: "trash",
            okText: "Удалить папку",
            onConfirm: async () => {
              try {
                await invoke("fs_delete_entry", { project: apkKitchenProject, relPath: d.folder });
                toast(`Папка ${d.name} удалена`);
                apkRefreshAll();
              } catch (e) { toast("Ошибка удаления: " + e); }
            }
          });
        };
        decBody.appendChild(tr);
      });
    } catch (e) {
      decBody.innerHTML = `<tr><td colspan="4" style="text-align:center;padding:16px;color:#ff8fab">Ошибка: ${e}</td></tr>`;
    }
  }

  $("#btnApkPickExternal").onclick = async () => {
    if (!apkKitchenProject) return toast("Сначала выберите проект!", "pink");
    if (T && T.dialog) {
      const file = await T.dialog.open({
        multiple: false,
        filters: [{ name: 'Android приложения', extensions: ['apk'] }]
      });
      if (file) {
        try {
          await invoke("decompile_apk", { project: apkKitchenProject, apkPath: file });
          toast("Запущена декомпиляция APK...", "pink");
        } catch (e) { toast("Ошибка: " + e); }
      }
    }
  };

  // Переход в файловый менеджер с раскрытием пути (например extracted/apk_Settings)
  async function fmRevealPath(project, relPath) {
    go("files");
    if (fmCurrentProject !== project) {
      fmCurrentProject = project;
      $("#fmProjectSelect").value = project;
      fmCurrentFile = null;
      fmExpandedFolders.clear();
      resetFmEditor();
    }
    await loadFmTree();
    const segs = relPath.split("/").filter(Boolean);
    let prefix = "";
    for (let i = 0; i < segs.length - 1; i++) {
      prefix = prefix ? prefix + "/" + segs[i] : segs[i];
      if (fmExpandedFolders.has(prefix)) continue;
      const row = document.querySelector(`.fm-row[data-path="${prefix}"]`);
      if (!row) break;
      const nameEl = row.querySelector(".fm-name");
      if (nameEl) nameEl.click();
      await sleep(250);
    }
    await sleep(150);
    const target = document.querySelector(`.fm-row[data-path="${relPath}"]`);
    if (target) {
      target.scrollIntoView({ block: "center" });
      target.style.background = "rgba(245,163,199,0.12)";
      setTimeout(() => { target.style.background = ""; }, 1600);
    }
  }

  /* ---------- WSL ---------- */
  async function checkWsl() {
    const ok = await invoke("wsl_status");
    $("#pillWsl").className = "pill " + (ok ? "ok" : "bad");
    $("#wslText").textContent = ok ? "WSL установлен и доступен." : "WSL не найден. Не обязателен — нужен только для отдельных специфичных Linux-скриптов.";
    $("#wslInstall").classList.toggle("hidden", !!ok);
  }
  $("#wslInstall").onclick = async () => {
    try { await invoke("wsl_install"); toast("Подтвердите запрос UAC. После установки нужна перезагрузка.", "pink"); }
    catch (e) { toast("Ошибка: " + e); }
  };

  /* ---------- старт приложения ---------- */
  async function boot() {
    const t0 = Date.now();
    // Ограничиваем проверку обновлений таймаутом в 2.5 секунды, чтобы сплэш не зависал
    const updatePromise = Promise.race([
      checkUpdate().catch(() => null),
      new Promise(resolve => setTimeout(() => resolve(null), 2500))
    ]);

    let u = null;
    try {
      u = await updatePromise;
    } catch (_) {}

    await sleep(Math.max(0, 800 - (Date.now() - t0)));

    if (u && u.available) {
      $("#splashImg").src = "assets/update.jpg";
      $(".progress").classList.add("hidden");
      $("#splashText").classList.add("hidden");
      $("#updVer").textContent = u.latest;
      $("#updNotes").textContent = u.notes || "";
      $("#updateBox").classList.remove("hidden");
      await new Promise((res) => {
        $("#updSkip").onclick = res;
        $("#updGo").onclick = async () => {
          const btn = $("#updGo");
          btn.disabled = true;
          btn.textContent = "Скачивание и установка…";
          try {
            await invoke("install_update"); // перезапустит приложение
          } catch (e) {
            toast("Ошибка обновления: " + e, "pink");
            if (u.url) invoke("open_url", { url: u.url });
            res();
          }
        };
      });
    } else {
      $("#splashText").textContent = u && u.error ? "GitHub недоступен — офлайн режим" : "Последняя версия";
      await sleep(350);
    }

    $("#splash").classList.add("out");
    $("#app").classList.remove("hidden");
    setTimeout(() => {
      const sp = $("#splash");
      if (sp) sp.remove();
    }, 700);

    requestAnimationFrame(() => go("home"));

    line("MEYAFORGE v0.1.1 запущен. Стек Rust + Clean WebUI готов.", "sys");
    const p = await invoke("get_profile").catch(() => null);
    applyProfile(p);
    if (!p) setTimeout(openProfile, 450);
    checkWsl();
    loadSavedDevicesCount();
    loadTelegramConfig();
  }
  boot();
})();
