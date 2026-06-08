(function () {
  function wrap(elements) {
    return {
      text(value) {
        elements.forEach((element) => {
          element.textContent = value;
        });
        return this;
      },
      html(value) {
        elements.forEach((element) => {
          element.innerHTML = value;
        });
        return this;
      },
      css(values) {
        elements.forEach((element) => {
          Object.assign(element.style, values);
        });
        return this;
      },
    };
  }

  function $(selectorOrReady) {
    if (typeof selectorOrReady === "function") {
      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", selectorOrReady);
      } else {
        selectorOrReady();
      }
      return;
    }
    return wrap(Array.from(document.querySelectorAll(selectorOrReady)));
  }

  $.getJSON = async function (url) {
    const response = await fetch(url, {
      headers: { Accept: "application/json" },
    });
    if (!response.ok) {
      throw new Error(`${url} returned ${response.status}`);
    }
    return response.json();
  };

  window.$ = $;
})();
