'use strict';

function Model(initial_params, csrf_blob, save_allowed) {
    const self = this
    observer.observable(self)

    self.params = {}
    Object.assign(self.params, initial_params)
    self.csrf_blob = csrf_blob
    self.save_allowed = save_allowed

    self.set = function(param, newvalue) {
        self.params[param] = newvalue
        self.emit("edit", param, self.params[param])
    }

    self.adjust = function(param, adjustment) {
        self.params[param] += adjustment
        self.emit("edit", param, self.params[param])
    }

    self.save = function() {
        self.emit("status", "Saving...")

        const post_json = {}
        post_json.params = self.params
        console.log(post_json)

        fetch("update",
            {method: "POST",
            body: JSON.stringify(post_json)})
        .then(response => {
            if (response.ok) {
                self.emit("status", "Saved")
            } else {
                // seriously?
                response.blob()
                .then(doneblob => {
                    doneblob.text()
                    .then(donetext => {
                        self.emit("status", 
                            "Failed: "  + response.status + ' '
                            + response.statusText + ' ' + donetext)
                    })
                })
            }
        })
    }
} // end model

// Presenter
(function() {

const initial_params = {{status.params|json|safe}}
const numinputs = {{numinputs|json|safe}}
const yesnoinputs = {{yesnoinputs|json|safe}}

window.model = new Model(initial_params, "{{csrf_blob}}", {{allowed}});

// make a map
const allinputs = {}
numinputs.forEach(input => { allinputs[input.name] = input } )
yesnoinputs.forEach(input => { allinputs[input.name] = input } )

function fixed_value(param, value) {
    return Number(value).toFixed(allinputs[param].digits)
}

model.on("edit", function(param, value) {
    const el = document.querySelector("#input_"+param);
    var same;
    switch (typeof(value)) {
        case "boolean":
            set_yesnoinput_value(el, value);
            same = ((!value) == (!initial_params[param]));
            break;
        case "number":
            same = (fixed_value(param, value) == fixed_value(param, initial_params[param]))
            set_numinput_value(el, param, value);
            break;
    }

    if (same) {
        el.querySelector(".oldvalue").classList.remove("modified")
    } else {
        el.querySelector(".oldvalue").classList.add("modified")
    }
})

model.on("status", function(status) {
    document.querySelector("#status").textContent = status
})

function set_numinput_value(el, name, value) {
    el.querySelector(".input").value = fixed_value(name, value)
};

function set_yesnoinput_value(el, value) {
    if (value) {
        el.querySelector(".button_yes").classList.add("onbutton")
        el.querySelector(".button_no").classList.remove("onbutton")
    } else {
        el.querySelector(".button_no").classList.add("onbutton")
        el.querySelector(".button_yes").classList.remove("onbutton")
    }
};

// View handler code
function setup_numinput(input) {
    const name = input.name
    const el = document.querySelector("#input_"+name)
    const inel = el.querySelector(".input")
    inel.addEventListener("keyup", function(e) {
        if (e.which == 13) {
            model.set(name, Number(this.value));
        }
    })

    inel.addEventListener("blur", function(e) {
        model.set(name, Number(this.value));
    })

    function uppress(e) {
        e.preventDefault()
        model.adjust(name, input.step)
        this.blur()
    }
    function downpress(e) {
        e.preventDefault()
        model.adjust(name, -input.step)
        this.blur()
    }
    el.querySelector(".button_up").addEventListener("mousedown", uppress)
    el.querySelector(".button_up").addEventListener("touchstart", uppress)
    el.querySelector(".button_down").addEventListener("mousedown", downpress)
    el.querySelector(".button_down").addEventListener("touchstart", downpress)

    el.querySelector(".oldvalue").textContent = fixed_value(name, model.params[name]) + input.unit
    set_numinput_value(el, name, model.params[name])
}

function setup_yesnoinput(input) {
    const name = input.name
    const el = document.querySelector("#input_"+name);
    function yespress(e) {
        model.set(name, true);
        this.blur()
    }
    function nopress(e) {
        model.set(name, false);
        this.blur()
    }
    el.querySelector(".button_yes").addEventListener("mousedown", yespress)
    el.querySelector(".button_yes").addEventListener("touchstart", yespress)
    el.querySelector(".button_no").addEventListener("mousedown", nopress)
    el.querySelector(".button_no").addEventListener("touchstart", nopress)

    el.querySelector(".oldvalue").textContent = model.params[name] ? 'Yes' : 'No'
    set_yesnoinput_value(el, model.params[name]);
}

window.addEventListener('DOMContentLoaded', (event) => {
    // Hook up events
    if (!model.save_allowed) {
        document.querySelector("#status").textContent = "No cert"
    }

    document.querySelector("#savebutton").addEventListener("click", function() {
        model.save();
    })

    numinputs.forEach(input => setup_numinput(input))
    yesnoinputs.forEach(input => setup_yesnoinput(input))
})

})() // end presenter


